//! Capture One-grade Levels, 6-Channel Black & White Mixer, Dehaze,
//! Auto/Eyedropper Color Science, and RGB/HSL/CIE Lab readouts.

use serde::{Deserialize, Serialize};

use crate::{rgb_to_hsv, srgb_to_linear};

const fn default_one() -> f32 {
    1.0
}

/// Capture One-style Input/Output Levels controls.
///
/// Evaluated in perceptual gamma-2.2 tone space with HDR headroom preservation
/// when `out_white == 1.0`, matching `apply_levels` in `shaders/develop.wgsl`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LevelsParams {
    /// Input black point (`0.0..=0.95`, default `0.0`).
    #[serde(default)]
    pub in_black: f32,
    /// Input midtone gamma shift (`-1.0..=1.0`, default `0.0` = gamma 1.0).
    #[serde(default)]
    pub in_mid: f32,
    /// Input white point (`0.05..=1.0`, default `1.0`).
    #[serde(default = "default_one")]
    pub in_white: f32,
    /// Output black point (`0.0..=1.0`, default `0.0`).
    #[serde(default)]
    pub out_black: f32,
    /// Output white point (`0.0..=1.0`, default `1.0`).
    #[serde(default = "default_one")]
    pub out_white: f32,
}

impl Default for LevelsParams {
    fn default() -> Self {
        Self {
            in_black: 0.0,
            in_mid: 0.0,
            in_white: 1.0,
            out_black: 0.0,
            out_white: 1.0,
        }
    }
}

impl LevelsParams {
    /// Returns `true` when Levels is an identity pass-through.
    pub fn is_identity(&self) -> bool {
        let s = self.sanitized();
        s.in_black.abs() <= 1e-6
            && s.in_mid.abs() <= 1e-6
            && (s.in_white - 1.0).abs() <= 1e-6
            && s.out_black.abs() <= 1e-6
            && (s.out_white - 1.0).abs() <= 1e-6
    }

    /// Sanitizes non-finite values and guarantees `in_white - in_black >= 0.01`.
    pub fn sanitized(self) -> Self {
        let mut in_black = if self.in_black.is_finite() {
            self.in_black.clamp(0.0, 0.95)
        } else {
            0.0
        };
        let in_mid = if self.in_mid.is_finite() {
            self.in_mid.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let mut in_white = if self.in_white.is_finite() {
            self.in_white.clamp(0.05, 1.0)
        } else {
            1.0
        };
        if in_white - in_black < 0.01 {
            in_white = (in_black + 0.01).min(1.0);
            in_black = (in_white - 0.01).max(0.0);
        }
        let out_black = if self.out_black.is_finite() {
            self.out_black.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let out_white = if self.out_white.is_finite() {
            self.out_white.clamp(0.0, 1.0)
        } else {
            1.0
        };
        Self {
            in_black,
            in_mid,
            in_white,
            out_black,
            out_white,
        }
    }
}

/// Capture One-style 6-Channel Black & White color sensitivity mixer.
///
/// Each channel offset lies in `-1.0..=1.0` (`0.0` = neutral Rec.709 luminance).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BlackAndWhiteParams {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub red: f32,
    #[serde(default)]
    pub yellow: f32,
    #[serde(default)]
    pub green: f32,
    #[serde(default)]
    pub cyan: f32,
    #[serde(default)]
    pub blue: f32,
    #[serde(default)]
    pub magenta: f32,
}

impl BlackAndWhiteParams {
    pub fn is_neutral(&self) -> bool {
        !self.enabled
            && self.red == 0.0
            && self.yellow == 0.0
            && self.green == 0.0
            && self.cyan == 0.0
            && self.blue == 0.0
            && self.magenta == 0.0
    }

    #[inline]
    pub fn channels_clamped(&self) -> [f32; 6] {
        let clamp = |v: f32| {
            if v.is_finite() {
                v.clamp(-1.0, 1.0)
            } else {
                0.0
            }
        };
        [
            clamp(self.red),
            clamp(self.yellow),
            clamp(self.green),
            clamp(self.cyan),
            clamp(self.blue),
            clamp(self.magenta),
        ]
    }

    /// Packs `[[enabled, red, yellow, green], [cyan, blue, magenta, 0.0]]` for GPU uniforms.
    #[inline]
    pub fn to_uniforms(&self) -> [[f32; 4]; 2] {
        let c = self.channels_clamped();
        [
            [if self.enabled { 1.0 } else { 0.0 }, c[0], c[1], c[2]],
            [c[3], c[4], c[5], 0.0],
        ]
    }
}

/// Evaluates `LevelsParams` in perceptual gamma-2.2 space with HDR headroom preservation
/// (exact parity with `apply_levels` in `shaders/develop.wgsl`).
#[inline]
pub(crate) fn apply_levels(rgb: [f32; 3], levels: LevelsParams) -> [f32; 3] {
    const INV_GAMMA: f32 = 1.0 / 2.2;
    const GAMMA: f32 = 2.2;

    let span = (levels.in_white - levels.in_black).max(0.01);
    let gamma = 2.0_f32.powf(-levels.in_mid.clamp(-1.0, 1.0) * 1.5);
    let hdr_keep = ((levels.out_white - 0.95) * 20.0).clamp(0.0, 1.0);
    let out_span = levels.out_white - levels.out_black;

    let mut out = [0.0_f32; 3];
    for i in 0..3 {
        let p = rgb[i].max(0.0).powf(INV_GAMMA);
        let norm = ((p - levels.in_black) / span).max(0.0);
        let base = norm.min(1.0).powf(gamma);
        let hdr = (norm - 1.0).max(0.0) * hdr_keep;
        let mapped = levels.out_black + base * out_span + hdr;
        out[i] = mapped.max(0.0).powf(GAMMA);
    }
    out
}

/// Single-pass dark-channel airlight Dehaze (positive removes haze, negative adds veil),
/// matching `apply_dehaze` in `shaders/develop.wgsl`.
#[inline]
pub(crate) fn apply_dehaze(rgb: [f32; 3], dehaze: f32) -> [f32; 3] {
    let d = if dehaze.is_finite() {
        dehaze.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    if d.abs() <= 1e-6 {
        return rgb;
    }

    let luma = (0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]).max(0.0);
    if d > 0.0 {
        let min_c = rgb[0].min(rgb[1]).min(rgb[2]);
        let veil = min_c * d * 0.45 / (1.0 + luma * 2.0);
        let trans = 1.0 - d * 0.24;
        let pivot = 0.18_f32;
        let c_exp = 1.0 + d * 0.18;
        [
            pivot * (((rgb[0] - veil).max(0.0) / trans) / pivot).powf(c_exp),
            pivot * (((rgb[1] - veil).max(0.0) / trans) / pivot).powf(c_exp),
            pivot * (((rgb[2] - veil).max(0.0) / trans) / pivot).powf(c_exp),
        ]
    } else {
        let h = -d;
        let airlight = [0.42_f32, 0.45, 0.50];
        let w = (h * 0.48 / (1.0 + luma * 1.4)).clamp(0.0, 0.85);
        [
            rgb[0] + (airlight[0] - rgb[0]) * w,
            rgb[1] + (airlight[1] - rgb[1]) * w,
            rgb[2] + (airlight[2] - rgb[2]) * w,
        ]
    }
}

/// Capture One 6-channel Black & White mixer (`R, Y, G, C, B, M` every 60°),
/// matching `apply_bw_mixer` in `shaders/develop.wgsl`.
#[inline]
pub(crate) fn apply_bw_mixer(rgb: [f32; 3], channels: [f32; 6]) -> [f32; 3] {
    const INV_GAMMA: f32 = 1.0 / 2.2;
    let base_y = (0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]).max(0.0);
    let p = [
        rgb[0].max(0.0).powf(INV_GAMMA),
        rgb[1].max(0.0).powf(INV_GAMMA),
        rgb[2].max(0.0).powf(INV_GAMMA),
    ];
    let [h_deg, s, _] = rgb_to_hsv(p);

    let sector_f = h_deg / 60.0;
    let i0 = (sector_f.floor() as usize) % 6;
    let i1 = (i0 + 1) % 6;
    let frac = (sector_f - sector_f.floor()).clamp(0.0, 1.0);
    let w1 = frac * frac * (3.0 - 2.0 * frac);
    let w0 = 1.0 - w1;
    let sens = channels[i0] * w0 + channels[i1] * w1;

    let y_out = (base_y * 2.0_f32.powf(sens * 1.5 * s)).max(0.0);
    [y_out, y_out, y_out]
}

/// Computes `(temperature, tint)` in `-1.0..=1.0` that neutralizes a clicked scene-linear pixel
/// under Rune's white balance gain model.
pub fn pick_white_balance(linear_rgb: [f32; 3]) -> (f32, f32) {
    if !linear_rgb.iter().all(|v| v.is_finite()) {
        return (0.0, 0.0);
    }
    let max_c = linear_rgb[0].max(linear_rgb[1]).max(linear_rgb[2]);
    if max_c <= 1e-5 {
        return (0.0, 0.0);
    }
    let r = linear_rgb[0].max(1e-5);
    let g = linear_rgb[1].max(1e-5);
    let b = linear_rgb[2].max(1e-5);

    // Closed-form inverse of:
    //   wb_r = 2^(temp * 0.45 + tint * 0.22)
    //   wb_g = 2^(-tint * 0.35)
    //   wb_b = 2^(-temp * 0.45 + tint * 0.22)
    let temp = (b / r).log2() / 0.90;
    let tint = (g.log2() - 0.5 * (r.log2() + b.log2())) / 0.57;
    (temp.clamp(-1.0, 1.0), tint.clamp(-1.0, 1.0))
}

/// Automatic specular-aware neutral White Balance from a linear RGB proxy.
///
/// Returns `(temperature, tint)` in `-1.0..=1.0`.
pub fn auto_white_balance(proxy: &[[f32; 3]]) -> (f32, f32) {
    let mut neutral_sum = [0.0_f64; 3];
    let mut neutral_n = 0u64;
    let mut broad_sum = [0.0_f64; 3];
    let mut broad_n = 0u64;

    for &px in proxy {
        if !px.iter().all(|v| v.is_finite() && *v >= 0.0) {
            continue;
        }
        let max_c = px[0].max(px[1]).max(px[2]);
        let min_c = px[0].min(px[1]).min(px[2]);
        let luma = 0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2];
        if luma < 0.005 || max_c > 0.95 {
            continue;
        }
        broad_sum[0] += px[0] as f64;
        broad_sum[1] += px[1] as f64;
        broad_sum[2] += px[2] as f64;
        broad_n += 1;

        let sat = if max_c > 1e-5 {
            (max_c - min_c) / max_c
        } else {
            0.0
        };
        if luma >= 0.015 && max_c <= 0.92 && sat <= 0.55 {
            neutral_sum[0] += px[0] as f64;
            neutral_sum[1] += px[1] as f64;
            neutral_sum[2] += px[2] as f64;
            neutral_n += 1;
        }
    }

    let (sum, n) = if neutral_n >= 16 {
        (neutral_sum, neutral_n)
    } else if broad_n > 0 {
        (broad_sum, broad_n)
    } else {
        return (0.0, 0.0);
    };

    let inv = 1.0 / n as f64;
    pick_white_balance([
        (sum[0] * inv) as f32,
        (sum[1] * inv) as f32,
        (sum[2] * inv) as f32,
    ])
}

/// Computes automatic `(exposure, contrast, highlights, shadows)` from a linear RGB proxy.
pub fn auto_exposure_and_tone(proxy: &[[f32; 3]]) -> (f32, f32, f32, f32) {
    let mut lumas: Vec<f32> = proxy
        .iter()
        .filter_map(|px| {
            if px.iter().all(|v| v.is_finite() && *v >= 0.0) {
                Some(0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2])
            } else {
                None
            }
        })
        .collect();
    if lumas.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    lumas.sort_by(f32::total_cmp);

    let at = |q: f32| -> f32 {
        let idx = ((lumas.len() - 1) as f32 * q).round() as usize;
        lumas[idx.min(lumas.len() - 1)]
    };

    let p05 = at(0.05);
    let p50 = at(0.50);
    let p95 = at(0.95);
    let p99 = at(0.99);

    // Target 18% middle gray while guarding against blowing out p99 highlights.
    let mut exposure = (0.18 / p50.max(0.005)).log2().clamp(-2.5, 2.5);
    if p99 > 1e-4 {
        let max_safe_exp = (1.35 / p99).log2();
        if exposure > max_safe_exp {
            exposure = (exposure * 0.5 + max_safe_exp * 0.5).clamp(-2.5, 2.5);
        }
    }

    let gain = 2.0_f32.powf(exposure);
    let hi = p99 * gain;
    let sh = p05 * gain;

    let highlights = if hi > 0.85 {
        -((hi - 0.85) * 0.65).clamp(0.0, 0.75)
    } else {
        0.0
    };
    let shadows = if sh < 0.025 {
        (((0.025 - sh) / 0.025) * 0.45).clamp(0.0, 0.65)
    } else {
        0.0
    };

    let dr_stops = (p95.max(1e-4) / p05.max(1e-4)).log2();
    let contrast = ((5.0 - dr_stops) * 0.08).clamp(-0.25, 0.35);

    (exposure, contrast, highlights, shadows)
}

/// Capture One-style Auto Levels using a `0.1%` shadow/highlight clipping threshold.
pub fn auto_levels(proxy: &[[f32; 3]]) -> LevelsParams {
    const BINS: usize = 1024;
    const INV_GAMMA: f32 = 1.0 / 2.2;

    let mut hist = [0u32; BINS];
    let mut total = 0u64;
    let max_bin = (BINS - 1) as f32;

    for &px in proxy {
        if !px.iter().all(|v| v.is_finite() && *v >= 0.0) {
            continue;
        }
        for c in px {
            let p = c.powf(INV_GAMMA).clamp(0.0, 1.0);
            let idx = (p * max_bin).round() as usize;
            hist[idx.min(BINS - 1)] += 1;
            total += 1;
        }
    }

    if total == 0 {
        return LevelsParams::default();
    }

    let clip_target = ((total as f64) * 0.001).max(1.0) as u64;

    let mut acc = 0u64;
    let mut low_bin = 0usize;
    for (i, &count) in hist.iter().enumerate() {
        acc += count as u64;
        if acc >= clip_target {
            low_bin = i;
            break;
        }
    }

    let mut acc_hi = 0u64;
    let mut high_bin = BINS - 1;
    for (i, &count) in hist.iter().enumerate().rev() {
        acc_hi += count as u64;
        if acc_hi >= clip_target {
            high_bin = i;
            break;
        }
    }

    let in_black = low_bin as f32 / max_bin;
    let in_white = high_bin as f32 / max_bin;
    if in_white - in_black < 0.05 {
        return LevelsParams::default();
    }

    LevelsParams {
        in_black,
        in_mid: 0.0,
        in_white,
        out_black: 0.0,
        out_white: 1.0,
    }
    .sanitized()
}

/// Converts an sRGB pixel (`[0.0..=1.0; 3]`) into `(HSL, CIE Lab)` for Capture One's
/// live Color Readout:
/// - `HSL`: `[H (0..360°), S (0..100%), L (0..100%)]`
/// - `CIE Lab` (D65): `[L* (0..100), a* (~-128..127), b* (~-128..127)]`
pub fn rgb_to_hsl_and_lab(srgb: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let r = if srgb[0].is_finite() {
        srgb[0].clamp(0.0, 1.0)
    } else {
        0.0
    };
    let g = if srgb[1].is_finite() {
        srgb[1].clamp(0.0, 1.0)
    } else {
        0.0
    };
    let b = if srgb[2].is_finite() {
        srgb[2].clamp(0.0, 1.0)
    } else {
        0.0
    };

    // 1. HSL
    let max_c = r.max(g).max(b);
    let min_c = r.min(g).min(b);
    let delta = max_c - min_c;
    let l = 0.5 * (max_c + min_c);
    let s = if delta <= 1e-6 {
        0.0
    } else {
        (delta / (1.0 - (2.0 * l - 1.0).abs()).max(1e-6)).clamp(0.0, 1.0)
    };
    let h = if delta <= 1e-6 {
        0.0
    } else if max_c == r {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if max_c == g {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    }
    .rem_euclid(360.0);

    // 2. sRGB -> Linear -> CIE XYZ (D65) -> CIE L*a*b* (D65)
    let lr = srgb_to_linear(r);
    let lg = srgb_to_linear(g);
    let lb = srgb_to_linear(b);

    let x = 0.412_456_4 * lr + 0.357_576_1 * lg + 0.180_437_5 * lb;
    let y = 0.212_672_9 * lr + 0.715_152_2 * lg + 0.072_175_0 * lb;
    let z = 0.019_333_9 * lr + 0.119_192 * lg + 0.950_304_1 * lb;

    #[inline]
    fn lab_f(t: f32) -> f32 {
        const DELTA: f32 = 6.0 / 29.0;
        const DELTA3: f32 = DELTA * DELTA * DELTA;
        const INV_3_DELTA2: f32 = 1.0 / (3.0 * DELTA * DELTA);
        if t > DELTA3 {
            t.cbrt()
        } else {
            t * INV_3_DELTA2 + 4.0 / 29.0
        }
    }

    let fx = lab_f(x / 0.95047);
    let fy = lab_f(y);
    let fz = lab_f(z / 1.08883);

    let l_star = (116.0 * fy - 16.0).clamp(0.0, 100.0);
    let a_star = 500.0 * (fx - fy);
    let b_star = 200.0 * (fy - fz);

    ([h, s * 100.0, l * 100.0], [l_star, a_star, b_star])
}
