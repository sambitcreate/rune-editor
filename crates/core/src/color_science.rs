//! Capture One-grade Levels, 6-Channel Black & White Mixer, Dehaze,
//! Auto/Eyedropper Color Science, and RGB/HSL/CIE Lab readouts.

use serde::{Deserialize, Serialize};

use crate::{hsv_to_rgb, rgb_to_hsv, srgb_to_linear};

/// Number of customizable color range slices in the Capture One Advanced Color Editor.
pub const ADVANCED_COLOR_SLICE_COUNT: usize = 4;

const fn default_one() -> f32 {
    1.0
}

const fn default_half() -> f32 {
    0.5
}

const fn default_hue_range() -> f32 {
    30.0
}

const fn default_sat_range() -> f32 {
    0.45
}

const fn default_slice_smoothness() -> f32 {
    0.4
}

const fn default_skin_hue() -> f32 {
    24.0
}

const fn default_skin_sat() -> f32 {
    0.42
}

const fn default_skin_lightness() -> f32 {
    0.62
}

const fn default_skin_smoothness() -> f32 {
    0.45
}

/// Single selectable color-range slice for the Capture One Advanced Color Editor.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorEditorSlice {
    #[serde(default)]
    pub enabled: bool,
    /// Target center hue in degrees (`0.0..=360.0`, default `0.0`).
    #[serde(default)]
    pub target_hue: f32,
    /// Target center saturation (`0.0..=1.0`, default `0.5`).
    #[serde(default = "default_half")]
    pub target_sat: f32,
    /// Half-width hue wedge in degrees (`5.0..=180.0`, default `30.0`).
    #[serde(default = "default_hue_range")]
    pub hue_range: f32,
    /// Half-width saturation range (`0.05..=1.0`, default `0.45`).
    #[serde(default = "default_sat_range")]
    pub sat_range: f32,
    /// Selection transition edge smoothness (`0.0..=1.0`, default `0.4`).
    #[serde(default = "default_slice_smoothness")]
    pub smoothness: f32,
    /// Hue rotation shift (`-1.0..=1.0`, ±60°, default `0.0`).
    #[serde(default)]
    pub hue_shift: f32,
    /// Saturation shift (`-1.0..=1.0`, default `0.0`).
    #[serde(default)]
    pub saturation: f32,
    /// Lightness shift (`-1.0..=1.0`, default `0.0`).
    #[serde(default)]
    pub lightness: f32,
    /// Smoothly pulls in-range pixels' hue & saturation toward `(target_hue, target_sat)` (`0.0..=1.0`).
    #[serde(default)]
    pub uniformity: f32,
}

impl Default for ColorEditorSlice {
    fn default() -> Self {
        Self {
            enabled: false,
            target_hue: 0.0,
            target_sat: default_half(),
            hue_range: default_hue_range(),
            sat_range: default_sat_range(),
            smoothness: default_slice_smoothness(),
            hue_shift: 0.0,
            saturation: 0.0,
            lightness: 0.0,
            uniformity: 0.0,
        }
    }
}

impl ColorEditorSlice {
    pub fn is_neutral(&self) -> bool {
        let d = Self::default();
        !self.enabled
            && (self.target_hue - d.target_hue).abs() <= 1e-5
            && (self.target_sat - d.target_sat).abs() <= 1e-5
            && (self.hue_range - d.hue_range).abs() <= 1e-5
            && (self.sat_range - d.sat_range).abs() <= 1e-5
            && (self.smoothness - d.smoothness).abs() <= 1e-5
            && self.hue_shift.abs() <= 1e-6
            && self.saturation.abs() <= 1e-6
            && self.lightness.abs() <= 1e-6
            && self.uniformity.abs() <= 1e-6
    }

    pub fn is_active(&self) -> bool {
        self.enabled
            && (self.hue_shift.abs() > 1e-6
                || self.saturation.abs() > 1e-6
                || self.lightness.abs() > 1e-6
                || self.uniformity > 1e-6)
    }

    pub fn sanitized(self) -> Self {
        let c = |v: f32, def: f32, lo: f32, hi: f32| {
            if v.is_finite() { v.clamp(lo, hi) } else { def }
        };
        Self {
            enabled: self.enabled,
            target_hue: c(self.target_hue, 0.0, 0.0, 360.0),
            target_sat: c(self.target_sat, default_half(), 0.0, 1.0),
            hue_range: c(self.hue_range, default_hue_range(), 5.0, 180.0),
            sat_range: c(self.sat_range, default_sat_range(), 0.05, 1.0),
            smoothness: c(self.smoothness, default_slice_smoothness(), 0.0, 1.0),
            hue_shift: c(self.hue_shift, 0.0, -1.0, 1.0),
            saturation: c(self.saturation, 0.0, -1.0, 1.0),
            lightness: c(self.lightness, 0.0, -1.0, 1.0),
            uniformity: c(self.uniformity, 0.0, 0.0, 1.0),
        }
    }

    /// Packs 3 `vec4<f32>` uniform slots for 16-byte aligned GPU uniforms.
    #[inline]
    pub fn to_uniforms(&self) -> [[f32; 4]; 3] {
        let s = self.sanitized();
        [
            [
                if s.enabled { 1.0 } else { 0.0 },
                s.target_hue,
                s.target_sat,
                s.hue_range,
            ],
            [s.sat_range, s.smoothness, s.hue_shift, s.saturation],
            [s.lightness, s.uniformity, 0.0, 0.0],
        ]
    }
}

/// Capture One-style Skin Tone Uniformity & Amount controls.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkinToneParams {
    #[serde(default)]
    pub enabled: bool,
    /// Target skin hue in degrees (`0.0..=360.0`, default `24.0`).
    #[serde(default = "default_skin_hue")]
    pub target_hue: f32,
    /// Target skin saturation (`0.0..=1.0`, default `0.42`).
    #[serde(default = "default_skin_sat")]
    pub target_sat: f32,
    /// Target skin lightness (`0.0..=1.0`, default `0.62`).
    #[serde(default = "default_skin_lightness")]
    pub target_lightness: f32,
    /// Selection mask transition smoothness (`0.0..=1.0`, default `0.45`).
    #[serde(default = "default_skin_smoothness")]
    pub smoothness: f32,
    /// Hue uniformity pulling skin hues toward `target_hue` (`0.0..=1.0`).
    #[serde(default)]
    pub uniformity_hue: f32,
    /// Saturation uniformity pulling skin saturation toward `target_sat` (`0.0..=1.0`).
    #[serde(default)]
    pub uniformity_sat: f32,
    /// Lightness uniformity pulling skin lightness toward `target_lightness` (`0.0..=1.0`).
    #[serde(default)]
    pub uniformity_lightness: f32,
    /// Skin hue shift (`-1.0..=1.0`).
    #[serde(default)]
    pub amount_hue: f32,
    /// Skin saturation shift (`-1.0..=1.0`).
    #[serde(default)]
    pub amount_sat: f32,
    /// Skin lightness shift (`-1.0..=1.0`).
    #[serde(default)]
    pub amount_lightness: f32,
}

impl Default for SkinToneParams {
    fn default() -> Self {
        Self {
            enabled: false,
            target_hue: default_skin_hue(),
            target_sat: default_skin_sat(),
            target_lightness: default_skin_lightness(),
            smoothness: default_skin_smoothness(),
            uniformity_hue: 0.0,
            uniformity_sat: 0.0,
            uniformity_lightness: 0.0,
            amount_hue: 0.0,
            amount_sat: 0.0,
            amount_lightness: 0.0,
        }
    }
}

impl SkinToneParams {
    pub fn is_neutral(&self) -> bool {
        let d = Self::default();
        !self.enabled
            && (self.target_hue - d.target_hue).abs() <= 1e-5
            && (self.target_sat - d.target_sat).abs() <= 1e-5
            && (self.target_lightness - d.target_lightness).abs() <= 1e-5
            && (self.smoothness - d.smoothness).abs() <= 1e-5
            && self.uniformity_hue.abs() <= 1e-6
            && self.uniformity_sat.abs() <= 1e-6
            && self.uniformity_lightness.abs() <= 1e-6
            && self.amount_hue.abs() <= 1e-6
            && self.amount_sat.abs() <= 1e-6
            && self.amount_lightness.abs() <= 1e-6
    }

    pub fn is_active(&self) -> bool {
        self.enabled
            && (self.uniformity_hue > 1e-6
                || self.uniformity_sat > 1e-6
                || self.uniformity_lightness > 1e-6
                || self.amount_hue.abs() > 1e-6
                || self.amount_sat.abs() > 1e-6
                || self.amount_lightness.abs() > 1e-6)
    }

    pub fn sanitized(self) -> Self {
        let c = |v: f32, def: f32, lo: f32, hi: f32| {
            if v.is_finite() { v.clamp(lo, hi) } else { def }
        };
        Self {
            enabled: self.enabled,
            target_hue: c(self.target_hue, default_skin_hue(), 0.0, 360.0),
            target_sat: c(self.target_sat, default_skin_sat(), 0.0, 1.0),
            target_lightness: c(self.target_lightness, default_skin_lightness(), 0.0, 1.0),
            smoothness: c(self.smoothness, default_skin_smoothness(), 0.0, 1.0),
            uniformity_hue: c(self.uniformity_hue, 0.0, 0.0, 1.0),
            uniformity_sat: c(self.uniformity_sat, 0.0, 0.0, 1.0),
            uniformity_lightness: c(self.uniformity_lightness, 0.0, 0.0, 1.0),
            amount_hue: c(self.amount_hue, 0.0, -1.0, 1.0),
            amount_sat: c(self.amount_sat, 0.0, -1.0, 1.0),
            amount_lightness: c(self.amount_lightness, 0.0, -1.0, 1.0),
        }
    }

    /// Packs 3 `vec4<f32>` uniform slots for 16-byte aligned GPU uniforms.
    #[inline]
    pub fn to_uniforms(&self) -> [[f32; 4]; 3] {
        let s = self.sanitized();
        [
            [
                if s.enabled { 1.0 } else { 0.0 },
                s.target_hue,
                s.target_sat,
                s.target_lightness,
            ],
            [
                s.smoothness,
                s.uniformity_hue,
                s.uniformity_sat,
                s.uniformity_lightness,
            ],
            [s.amount_hue, s.amount_sat, s.amount_lightness, 0.0],
        ]
    }
}

/// Samples perceptual `(hue_deg, saturation, lightness)` from a scene-referred linear RGB pixel
/// for Advanced Color Editor and Skin Tone Uniformity eyedropper picking.
pub fn sample_color_slice_from_rgb(linear_rgb: [f32; 3]) -> (f32, f32, f32) {
    if !linear_rgb.iter().all(|v| v.is_finite()) {
        return (0.0, 0.0, 0.0);
    }
    const INV_GAMMA: f32 = 1.0 / 2.2;
    let p = [
        linear_rgb[0].max(0.0).powf(INV_GAMMA),
        linear_rgb[1].max(0.0).powf(INV_GAMMA),
        linear_rgb[2].max(0.0).powf(INV_GAMMA),
    ];
    let [h, s, v] = rgb_to_hsv(p);
    (h.clamp(0.0, 360.0), s.clamp(0.0, 1.0), v.clamp(0.0, 1.0))
}

#[inline]
fn smoothstep_cs(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn signed_hue_diff_deg(h: f32, target: f32) -> f32 {
    (h - target + 540.0).rem_euclid(360.0) - 180.0
}

/// Evaluates the `[0.0, 1.0]` selection mask weight of an Advanced Color Editor slice
/// in perceptual gamma-2.2 HSV space (matches `advanced_slice_mask` in `shaders/develop.wgsl`).
#[inline]
pub(crate) fn advanced_slice_mask(h: f32, s: f32, slice: ColorEditorSlice) -> f32 {
    let diff_h = signed_hue_diff_deg(h, slice.target_hue).abs();
    let feather_h = (slice.hue_range * (0.15 + slice.smoothness * 1.15)).max(2.0);
    let inner_h = slice.hue_range * (1.0 - slice.smoothness * 0.55);
    let outer_h = slice.hue_range + feather_h * 0.5;
    let w_h = 1.0 - smoothstep_cs(inner_h, outer_h, diff_h);

    let diff_s = (s - slice.target_sat).abs();
    let inner_s = slice.sat_range * (1.0 - slice.smoothness * 0.55);
    let outer_s = slice.sat_range + (slice.sat_range * (0.15 + slice.smoothness * 0.85)).max(0.04);
    let w_s = 1.0 - smoothstep_cs(inner_s, outer_s, diff_s);

    let chroma_guard = if slice.target_sat > 0.05 {
        smoothstep_cs(0.015, 0.08, s)
    } else {
        1.0
    };
    (w_h * w_s * chroma_guard).clamp(0.0, 1.0)
}

/// Evaluates all 4 Advanced Color Editor slices in perceptual gamma-2.2 HSV space,
/// matching `apply_advanced_color` in `shaders/develop.wgsl`.
#[inline]
pub(crate) fn apply_advanced_color(
    rgb: [f32; 3],
    slices: &[ColorEditorSlice; ADVANCED_COLOR_SLICE_COUNT],
) -> [f32; 3] {
    const INV_GAMMA: f32 = 1.0 / 2.2;
    const GAMMA: f32 = 2.2;

    let p = [
        rgb[0].max(0.0).powf(INV_GAMMA),
        rgb[1].max(0.0).powf(INV_GAMMA),
        rgb[2].max(0.0).powf(INV_GAMMA),
    ];
    let [mut h, mut s, mut v] = rgb_to_hsv(p);

    for &slice in slices {
        if !slice.is_active() {
            continue;
        }
        let w = advanced_slice_mask(h, s, slice);
        if w <= 1e-5 {
            continue;
        }
        let diff_h = signed_hue_diff_deg(h, slice.target_hue);
        h = (h - diff_h * slice.uniformity * 0.85 * w + slice.hue_shift * 60.0 * w)
            .rem_euclid(360.0);
        let s_u = s + (slice.target_sat - s) * (slice.uniformity * 0.75 * w);
        s = (s_u * (1.0 + slice.saturation * w)).clamp(0.0, 1.0);
        v = (v * 2.0_f32.powf(slice.lightness * 0.75 * w)).max(0.0);
    }

    let p_out = hsv_to_rgb([h, s, v]);
    [
        p_out[0].max(0.0).powf(GAMMA),
        p_out[1].max(0.0).powf(GAMMA),
        p_out[2].max(0.0).powf(GAMMA),
    ]
}

/// Evaluates the `[0.0, 1.0]` selection mask weight of Skin Tone Uniformity
/// in perceptual gamma-2.2 HSV space (matches `skin_tone_mask` in `shaders/develop.wgsl`).
#[inline]
pub(crate) fn skin_tone_mask(h: f32, s: f32, v: f32, skin: SkinToneParams) -> f32 {
    let diff_h = signed_hue_diff_deg(h, skin.target_hue).abs();
    let inner_h = 26.0 * (1.0 - skin.smoothness * 0.55);
    let outer_h = 26.0 + (26.0 * (0.20 + skin.smoothness * 1.1)).max(4.0);
    let w_h = 1.0 - smoothstep_cs(inner_h, outer_h, diff_h);

    let diff_s = (s - skin.target_sat).abs();
    let inner_s = 0.38 * (1.0 - skin.smoothness * 0.55);
    let outer_s = 0.38 + (0.38 * (0.20 + skin.smoothness * 0.85)).max(0.05);
    let w_s = 1.0 - smoothstep_cs(inner_s, outer_s, diff_s);

    let chroma_guard = smoothstep_cs(0.03, 0.12, s);
    let luma_guard = smoothstep_cs(0.03, 0.14, v);
    (w_h * w_s * chroma_guard * luma_guard).clamp(0.0, 1.0)
}

/// Evaluates Capture One Skin Tone Uniformity & Amount in perceptual gamma-2.2 HSV space,
/// matching `apply_skin_tone` in `shaders/develop.wgsl`.
#[inline]
pub(crate) fn apply_skin_tone(rgb: [f32; 3], skin: SkinToneParams) -> [f32; 3] {
    if !skin.is_active() {
        return rgb;
    }
    const INV_GAMMA: f32 = 1.0 / 2.2;
    const GAMMA: f32 = 2.2;

    let p = [
        rgb[0].max(0.0).powf(INV_GAMMA),
        rgb[1].max(0.0).powf(INV_GAMMA),
        rgb[2].max(0.0).powf(INV_GAMMA),
    ];
    let [h, s, v] = rgb_to_hsv(p);
    let w = skin_tone_mask(h, s, v, skin);
    if w <= 1e-5 {
        return rgb;
    }

    let diff_h = signed_hue_diff_deg(h, skin.target_hue);
    let h_new = (h - diff_h * skin.uniformity_hue * 0.85 * w + skin.amount_hue * 35.0 * w)
        .rem_euclid(360.0);
    let s_u = s + (skin.target_sat - s) * (skin.uniformity_sat * 0.80 * w);
    let s_new = (s_u * (1.0 + skin.amount_sat * 0.65 * w)).clamp(0.0, 1.0);
    let v_base = v.clamp(0.0, 1.0);
    let v_hdr = (v - 1.0).max(0.0);
    let v_u =
        v_base + (skin.target_lightness - v_base) * (skin.uniformity_lightness * 0.65 * w) + v_hdr;
    let v_new = (v_u * 2.0_f32.powf(skin.amount_lightness * 0.65 * w)).max(0.0);

    let p_out = hsv_to_rgb([h_new, s_new, v_new]);
    [
        p_out[0].max(0.0).powf(GAMMA),
        p_out[1].max(0.0).powf(GAMMA),
        p_out[2].max(0.0).powf(GAMMA),
    ]
}

/// Capture One-style White Balance illuminant presets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WbPreset {
    #[default]
    AsShot,
    Daylight,
    Cloudy,
    Shade,
    Tungsten,
    Fluorescent,
    Flash,
    Custom,
}

impl WbPreset {
    pub const ALL: [Self; 8] = [
        Self::AsShot,
        Self::Daylight,
        Self::Cloudy,
        Self::Shade,
        Self::Tungsten,
        Self::Fluorescent,
        Self::Flash,
        Self::Custom,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::AsShot => "As Shot",
            Self::Daylight => "Daylight",
            Self::Cloudy => "Cloudy",
            Self::Shade => "Shade",
            Self::Tungsten => "Tungsten",
            Self::Fluorescent => "Fluorescent",
            Self::Flash => "Flash",
            Self::Custom => "Custom",
        }
    }

    pub const fn cli_slug(self) -> &'static str {
        match self {
            Self::AsShot => "as_shot",
            Self::Daylight => "daylight",
            Self::Cloudy => "cloudy",
            Self::Shade => "shade",
            Self::Tungsten => "tungsten",
            Self::Fluorescent => "fluorescent",
            Self::Flash => "flash",
            Self::Custom => "custom",
        }
    }

    pub fn from_cli_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "as_shot" | "asshot" | "shot" => Some(Self::AsShot),
            "daylight" | "day" | "sun" => Some(Self::Daylight),
            "cloudy" | "cloud" | "overcast" => Some(Self::Cloudy),
            "shade" => Some(Self::Shade),
            "tungsten" | "incandescent" => Some(Self::Tungsten),
            "fluorescent" | "fluoro" => Some(Self::Fluorescent),
            "flash" | "strobe" => Some(Self::Flash),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

/// Maps normalized white balance `temperature` (`-1.0..=+1.0`) smoothly to Kelvin (`2000K..=14000K`)
/// with `0.0 -> 5500K` using piecewise log-Kelvin scaling.
pub fn temperature_to_kelvin(temp: f32) -> u32 {
    if !temp.is_finite() {
        return 5500;
    }
    let t = temp.clamp(-1.0, 1.0);
    let k = if t <= 0.0 {
        5500.0 * (5500.0_f32 / 2000.0).powf(t)
    } else {
        5500.0 * (14000.0_f32 / 5500.0).powf(t)
    };
    (k.round() as u32).clamp(2000, 14000)
}

/// Inverse of [`temperature_to_kelvin`]: maps Kelvin (`2000K..=14000K`) to normalized
/// white balance `temperature` (`-1.0..=+1.0`) with `5500K -> 0.0`.
pub fn kelvin_to_temperature(kelvin: u32) -> f32 {
    let k = (kelvin as f32).clamp(2000.0, 14000.0);
    if k <= 5500.0 {
        ((k / 5500.0).ln() / (5500.0_f32 / 2000.0).ln()).clamp(-1.0, 0.0)
    } else {
        ((k / 5500.0).ln() / (14000.0_f32 / 5500.0).ln()).clamp(0.0, 1.0)
    }
}

/// Returns `(temperature, tint)` for each standard white balance illuminant preset.
///
/// `AsShot` returns `Some((0.0, 0.0))` (camera neutral) and `Custom` returns `None`.
pub fn wb_preset_values(preset: WbPreset) -> Option<(f32, f32)> {
    match preset {
        WbPreset::AsShot => Some((0.0, 0.0)),
        WbPreset::Daylight => Some((kelvin_to_temperature(5500), 0.02)),
        WbPreset::Cloudy => Some((kelvin_to_temperature(6500), 0.04)),
        WbPreset::Shade => Some((kelvin_to_temperature(7500), 0.05)),
        WbPreset::Tungsten => Some((kelvin_to_temperature(3200), -0.02)),
        WbPreset::Fluorescent => Some((kelvin_to_temperature(3800), 0.22)),
        WbPreset::Flash => Some((kelvin_to_temperature(5800), 0.03)),
        WbPreset::Custom => None,
    }
}

/// Per-channel Input/Output Levels controls (same units and bounds as master [`LevelsParams`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelLevels {
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

impl Default for ChannelLevels {
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

impl ChannelLevels {
    /// Returns `true` when this channel's levels represent an identity pass-through.
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

/// Capture One-style Master RGB + per-channel R/G/B Input/Output Levels controls.
///
/// Evaluated in perceptual gamma-2.2 tone space with HDR headroom preservation
/// when `out_white == 1.0`, matching `apply_levels` in `shaders/develop.wgsl`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LevelsParams {
    /// Master RGB input black point (`0.0..=0.95`, default `0.0`).
    #[serde(default)]
    pub in_black: f32,
    /// Master RGB input midtone gamma shift (`-1.0..=1.0`, default `0.0` = gamma 1.0).
    #[serde(default)]
    pub in_mid: f32,
    /// Master RGB input white point (`0.05..=1.0`, default `1.0`).
    #[serde(default = "default_one")]
    pub in_white: f32,
    /// Master RGB output black point (`0.0..=1.0`, default `0.0`).
    #[serde(default)]
    pub out_black: f32,
    /// Master RGB output white point (`0.0..=1.0`, default `1.0`).
    #[serde(default = "default_one")]
    pub out_white: f32,
    /// Per-channel Red levels applied after master RGB levels.
    #[serde(default)]
    pub red: ChannelLevels,
    /// Per-channel Green levels applied after master RGB levels.
    #[serde(default)]
    pub green: ChannelLevels,
    /// Per-channel Blue levels applied after master RGB levels.
    #[serde(default)]
    pub blue: ChannelLevels,
}

impl Default for LevelsParams {
    fn default() -> Self {
        Self {
            in_black: 0.0,
            in_mid: 0.0,
            in_white: 1.0,
            out_black: 0.0,
            out_white: 1.0,
            red: ChannelLevels::default(),
            green: ChannelLevels::default(),
            blue: ChannelLevels::default(),
        }
    }
}

impl LevelsParams {
    /// Returns the master RGB levels as a [`ChannelLevels`] struct.
    #[inline]
    pub fn master_channel(&self) -> ChannelLevels {
        ChannelLevels {
            in_black: self.in_black,
            in_mid: self.in_mid,
            in_white: self.in_white,
            out_black: self.out_black,
            out_white: self.out_white,
        }
    }

    /// Returns `true` when master RGB levels are an identity pass-through.
    pub fn is_master_identity(&self) -> bool {
        self.master_channel().is_identity()
    }

    /// Returns `true` when master RGB and all per-channel R/G/B levels are identity.
    pub fn is_identity(&self) -> bool {
        self.is_master_identity()
            && self.red.is_identity()
            && self.green.is_identity()
            && self.blue.is_identity()
    }

    /// Sanitizes non-finite values and guarantees `in_white - in_black >= 0.01`
    /// for master RGB and every individual channel.
    pub fn sanitized(self) -> Self {
        let m = self.master_channel().sanitized();
        Self {
            in_black: m.in_black,
            in_mid: m.in_mid,
            in_white: m.in_white,
            out_black: m.out_black,
            out_white: m.out_white,
            red: self.red.sanitized(),
            green: self.green.sanitized(),
            blue: self.blue.sanitized(),
        }
    }
}

/// Capture One-style 6-Channel Black & White color sensitivity mixer + Split Toning.
///
/// Each channel offset lies in `-1.0..=1.0` (`0.0` = neutral Rec.709 luminance).
/// Split toning tints monochrome shadows and highlights with hues in `0.0..=360.0`
/// and saturations in `0.0..=1.0`.
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
    /// Split toning shadow hue in degrees (`0.0..=360.0`, default `0.0`).
    #[serde(default)]
    pub split_shadow_hue: f32,
    /// Split toning shadow saturation (`0.0..=1.0`, default `0.0`).
    #[serde(default)]
    pub split_shadow_sat: f32,
    /// Split toning highlight hue in degrees (`0.0..=360.0`, default `0.0`).
    #[serde(default)]
    pub split_highlight_hue: f32,
    /// Split toning highlight saturation (`0.0..=1.0`, default `0.0`).
    #[serde(default)]
    pub split_highlight_sat: f32,
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
            && self.split_shadow_hue == 0.0
            && self.split_shadow_sat == 0.0
            && self.split_highlight_hue == 0.0
            && self.split_highlight_sat == 0.0
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

    /// Returns clamped `[split_shadow_hue, split_shadow_sat, split_highlight_hue, split_highlight_sat]`.
    #[inline]
    pub fn split_tones_clamped(&self) -> [f32; 4] {
        let clamp_hue = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, 360.0)
            } else {
                0.0
            }
        };
        let clamp_sat = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        [
            clamp_hue(self.split_shadow_hue),
            clamp_sat(self.split_shadow_sat),
            clamp_hue(self.split_highlight_hue),
            clamp_sat(self.split_highlight_sat),
        ]
    }

    /// Returns a sanitized copy with all fields finite and clamped to valid bounds.
    pub fn sanitized(self) -> Self {
        let c = self.channels_clamped();
        let s = self.split_tones_clamped();
        Self {
            enabled: self.enabled,
            red: c[0],
            yellow: c[1],
            green: c[2],
            cyan: c[3],
            blue: c[4],
            magenta: c[5],
            split_shadow_hue: s[0],
            split_shadow_sat: s[1],
            split_highlight_hue: s[2],
            split_highlight_sat: s[3],
        }
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

    /// Packs `[split_shadow_hue, split_shadow_sat, split_highlight_hue, split_highlight_sat]`
    /// for 16-byte aligned GPU uniforms.
    #[inline]
    pub fn to_split_uniform(&self) -> [f32; 4] {
        self.split_tones_clamped()
    }
}

#[inline]
fn map_level_perceptual(p: f32, ch: ChannelLevels) -> f32 {
    let span = (ch.in_white - ch.in_black).max(0.01);
    let gamma = 2.0_f32.powf(-ch.in_mid.clamp(-1.0, 1.0) * 1.5);
    let hdr_keep = ((ch.out_white - 0.95) * 20.0).clamp(0.0, 1.0);
    let out_span = ch.out_white - ch.out_black;
    let norm = ((p - ch.in_black) / span).max(0.0);
    let base = norm.min(1.0).powf(gamma);
    let hdr = (norm - 1.0).max(0.0) * hdr_keep;
    (ch.out_black + base * out_span + hdr).max(0.0)
}

/// Evaluates master RGB and per-channel R/G/B `LevelsParams` in perceptual gamma-2.2 space
/// with HDR headroom preservation (exact parity with `apply_levels` in `shaders/develop.wgsl`).
#[inline]
pub(crate) fn apply_levels(rgb: [f32; 3], levels: LevelsParams) -> [f32; 3] {
    const INV_GAMMA: f32 = 1.0 / 2.2;
    const GAMMA: f32 = 2.2;

    let master = levels.master_channel();
    let master_active = !master.is_identity();
    let chs = [levels.red, levels.green, levels.blue];
    let ch_active = [
        !chs[0].is_identity(),
        !chs[1].is_identity(),
        !chs[2].is_identity(),
    ];

    let mut out = rgb;
    for i in 0..3 {
        if master_active || ch_active[i] {
            let mut p = rgb[i].max(0.0).powf(INV_GAMMA);
            if master_active {
                p = map_level_perceptual(p, master);
            }
            if ch_active[i] {
                p = map_level_perceptual(p, chs[i]);
            }
            out[i] = p.max(0.0).powf(GAMMA);
        }
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

/// Capture One 6-channel Black & White mixer (`R, Y, G, C, B, M` every 60°) + optional
/// Split Toning (`[shadow_hue, shadow_sat, highlight_hue, highlight_sat]`),
/// matching `apply_bw_mixer` in `shaders/develop.wgsl`.
#[inline]
pub(crate) fn apply_bw_mixer(rgb: [f32; 3], channels: [f32; 6], split_tones: [f32; 4]) -> [f32; 3] {
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

    let sh_sat = split_tones[1].clamp(0.0, 1.0);
    let hi_sat = split_tones[3].clamp(0.0, 1.0);
    if sh_sat <= 1e-5 && hi_sat <= 1e-5 {
        return [y_out, y_out, y_out];
    }

    let t = (y_out / (y_out + 0.18)).clamp(0.0, 1.0);
    let sh_rgb = hsv_to_rgb([split_tones[0], 1.0, 1.0]);
    let hi_rgb = hsv_to_rgb([split_tones[2], 1.0, 1.0]);
    let sh_y = 0.2126 * sh_rgb[0] + 0.7152 * sh_rgb[1] + 0.0722 * sh_rgb[2];
    let hi_y = 0.2126 * hi_rgb[0] + 0.7152 * hi_rgb[1] + 0.0722 * hi_rgb[2];

    let mut out = [0.0_f32; 3];
    for i in 0..3 {
        let tint_s = sh_rgb[i] - sh_y;
        let tint_h = hi_rgb[i] - hi_y;
        let split_tint = tint_s * (sh_sat * (1.0 - t)) + tint_h * (hi_sat * t);
        out[i] = (y_out * (1.0 + split_tint * 0.85)).max(0.0);
    }
    out
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
        ..LevelsParams::default()
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

const fn default_film_base_r() -> f32 {
    0.95
}

const fn default_film_base_g() -> f32 {
    0.56
}

const fn default_film_base_b() -> f32 {
    0.34
}

const fn default_film_black_clip() -> f32 {
    0.02
}

const fn default_film_white_clip() -> f32 {
    0.98
}

/// Capture One-style Base Characteristics processing mode (`Photography` vs `FilmNegative`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingMode {
    #[default]
    Photography,
    FilmNegative,
}

impl ProcessingMode {
    pub const ALL: [Self; 2] = [Self::Photography, Self::FilmNegative];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Photography => "Photography",
            Self::FilmNegative => "Film Negative",
        }
    }

    pub const fn cli_slug(self) -> &'static str {
        match self {
            Self::Photography => "photography",
            Self::FilmNegative => "film_negative",
        }
    }

    pub fn from_cli_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "photography" | "photo" | "positive" => Some(Self::Photography),
            "film_negative" | "negative" | "film" => Some(Self::FilmNegative),
            _ => None,
        }
    }

    #[inline]
    pub const fn to_uniform_f32(self) -> f32 {
        match self {
            Self::Photography => 0.0,
            Self::FilmNegative => 1.0,
        }
    }
}

/// Optical Film Negative Conversion parameters (C-41 orange mask neutralization, print density,
/// gamma contrast, and black/white rebate clipping).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilmNegativeParams {
    /// Film base orange-mask red linear reflectance (`0.05..=2.0`, default `0.95`).
    #[serde(default = "default_film_base_r")]
    pub base_r: f32,
    /// Film base orange-mask green linear reflectance (`0.05..=2.0`, default `0.56`).
    #[serde(default = "default_film_base_g")]
    pub base_g: f32,
    /// Film base orange-mask blue linear reflectance (`0.05..=2.0`, default `0.34`).
    #[serde(default = "default_film_base_b")]
    pub base_b: f32,
    /// Print density / exposure shift (`-1.0..=1.0`, default `0.0`).
    #[serde(default)]
    pub density: f32,
    /// Print gamma contrast (`-1.0..=1.0`, default `0.0`).
    #[serde(default)]
    pub contrast: f32,
    /// Shadow rebate black point clip (`0.0..=0.45`, default `0.02`).
    #[serde(default = "default_film_black_clip")]
    pub black_clip: f32,
    /// Dense highlight white point clip (`0.55..=1.0`, default `0.98`).
    #[serde(default = "default_film_white_clip")]
    pub white_clip: f32,
}

impl Default for FilmNegativeParams {
    fn default() -> Self {
        Self {
            base_r: default_film_base_r(),
            base_g: default_film_base_g(),
            base_b: default_film_base_b(),
            density: 0.0,
            contrast: 0.0,
            black_clip: default_film_black_clip(),
            white_clip: default_film_white_clip(),
        }
    }
}

impl FilmNegativeParams {
    pub fn is_default(&self) -> bool {
        let d = Self::default();
        (self.base_r - d.base_r).abs() <= 1e-5
            && (self.base_g - d.base_g).abs() <= 1e-5
            && (self.base_b - d.base_b).abs() <= 1e-5
            && self.density.abs() <= 1e-5
            && self.contrast.abs() <= 1e-5
            && (self.black_clip - d.black_clip).abs() <= 1e-5
            && (self.white_clip - d.white_clip).abs() <= 1e-5
    }

    pub fn sanitized(self) -> Self {
        let cf = |v: f32, def: f32, lo: f32, hi: f32| {
            if v.is_finite() { v.clamp(lo, hi) } else { def }
        };
        let black_clip = cf(self.black_clip, default_film_black_clip(), 0.0, 0.45);
        let white_clip = cf(self.white_clip, default_film_white_clip(), 0.55, 1.0)
            .max(black_clip + 0.10)
            .min(1.0);
        Self {
            base_r: cf(self.base_r, default_film_base_r(), 0.05, 2.0),
            base_g: cf(self.base_g, default_film_base_g(), 0.05, 2.0),
            base_b: cf(self.base_b, default_film_base_b(), 0.05, 2.0),
            density: cf(self.density, 0.0, -1.0, 1.0),
            contrast: cf(self.contrast, 0.0, -1.0, 1.0),
            black_clip,
            white_clip,
        }
    }

    /// Packs into two 16-byte aligned uniform vectors matching `shaders/develop.wgsl`:
    /// - `film_neg_a = [base_r, base_g, base_b, processing_mode (0/1)]`
    /// - `film_neg_b = [density, contrast, black_clip, white_clip]`
    #[inline]
    pub fn to_uniforms(&self, mode: ProcessingMode) -> ([f32; 4], [f32; 4]) {
        let s = self.sanitized();
        (
            [s.base_r, s.base_g, s.base_b, mode.to_uniform_f32()],
            [s.density, s.contrast, s.black_clip, s.white_clip],
        )
    }
}

/// Sanitizes a sampled raw linear RGB pixel from the film negative rebate border into safe
/// `[base_r, base_g, base_b]` orange-mask reference values (`0.05..=2.0`).
#[inline]
pub fn sample_film_base_from_rgb(raw_linear_rgb: [f32; 3]) -> [f32; 3] {
    let cf = |v: f32, def: f32| {
        if v.is_finite() {
            v.clamp(0.05, 2.0)
        } else {
            def
        }
    };
    [
        cf(raw_linear_rgb[0], default_film_base_r()),
        cf(raw_linear_rgb[1], default_film_base_g()),
        cf(raw_linear_rgb[2], default_film_base_b()),
    ]
}

/// Optical Stage 0.5 Film Negative Inversion matching `invert_film_negative` in `shaders/develop.wgsl`
/// bit-for-bit (`< 2e-3` GPU↔CPU parity, zero division by zero, strictly monotonic).
#[inline]
pub fn invert_film_negative_linear(rgb: [f32; 3], params: &FilmNegativeParams) -> [f32; 3] {
    let p = params.sanitized();
    let base = [p.base_r.max(0.05), p.base_g.max(0.05), p.base_b.max(0.05)];
    let span = (p.white_clip - p.black_clip).max(0.05);
    let gamma = 2.2 * 2.0_f32.powf(p.contrast * 0.65);
    let density_gain = 2.0_f32.powf(p.density * 1.5);

    let mut out = [0.0_f32; 3];
    for ch in 0..3 {
        let trans = (rgb[ch].max(0.0) / base[ch]).clamp(0.0, 1.0);
        let t_neg = trans.powf(1.0 / 2.2).clamp(0.0, 1.0);
        let inv = 1.0 - t_neg;
        let norm = ((inv - p.black_clip) / span).clamp(0.0, 1.0);
        out[ch] = (norm.powf(gamma) * density_gain).max(0.0);
    }
    out
}
