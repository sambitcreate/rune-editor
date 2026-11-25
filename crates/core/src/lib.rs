//! UI- and GPU-agnostic core for Rune Editor.
//!
//! Owns the non-destructive photographic develop recipe, sidecar serialization,
//! undo/redo history stack, built-in Capture One / Lightroom-style presets, and
//! scene-referred linear color math (including fast histogram evaluation).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Version tag embedded in `.rune-edit.json` sidecar files.
pub const RECIPE_SCHEMA_VERSION: u32 = 1;

/// Non-destructive develop parameters for a single photograph.
///
/// All tone & color operations evaluate in **scene-referred linear light**
/// (16-bit half-float `Rgba16Float`), independent of display output gamut.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DevelopRecipe {
    pub schema_version: u32,
    /// Exposure compensation in photographic stops (`-5.0..=+5.0`).
    pub exposure: f32,
    /// Filmic S-curve contrast around 18% middle gray (`-1.0..=+1.0`).
    pub contrast: f32,
    /// Highlight recovery / boost (`-1.0..=+1.0`).
    pub highlights: f32,
    /// Shadow lift / deepen (`-1.0..=+1.0`).
    pub shadows: f32,
    /// White point trim (`-1.0..=+1.0`).
    pub whites: f32,
    /// Black point trim (`-1.0..=+1.0`).
    pub blacks: f32,
    /// White balance temperature shift (`-1.0` cool blue .. `+1.0` warm amber).
    pub temperature: f32,
    /// White balance green/magenta tint (`-1.0` green .. `+1.0` magenta).
    pub tint: f32,
    /// Perceptual chroma boost protecting near-saturated & skin tones (`-1.0..=+1.0`).
    pub vibrance: f32,
    /// Global color saturation (`-1.0` monochrome .. `+1.0` vivid).
    pub saturation: f32,
    /// Optical radial vignette (`-1.0` dark edges .. `+1.0` bright edges).
    pub vignette: f32,
    /// Subtle photographic film grain amount (`0.0..=1.0`).
    pub grain: f32,
    /// Active preset name if unmodified from a style.
    pub preset_name: Option<String>,
}

impl Default for DevelopRecipe {
    fn default() -> Self {
        Self {
            schema_version: RECIPE_SCHEMA_VERSION,
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            temperature: 0.0,
            tint: 0.0,
            vibrance: 0.0,
            saturation: 0.0,
            vignette: 0.0,
            grain: 0.0,
            preset_name: Some("Neutral".to_owned()),
        }
    }
}

impl DevelopRecipe {
    pub fn is_neutral(&self) -> bool {
        let d = Self::default();
        self.exposure == d.exposure
            && self.contrast == d.contrast
            && self.highlights == d.highlights
            && self.shadows == d.shadows
            && self.whites == d.whites
            && self.blacks == d.blacks
            && self.temperature == d.temperature
            && self.tint == d.tint
            && self.vibrance == d.vibrance
            && self.saturation == d.saturation
            && self.vignette == d.vignette
            && self.grain == d.grain
    }

    pub fn sidecar_path_for(photo_path: &Path) -> PathBuf {
        let mut s = photo_path.as_os_str().to_os_string();
        s.push(".rune-edit.json");
        PathBuf::from(s)
    }

    pub fn save_sidecar(&self, photo_path: &Path) -> Result<PathBuf, CoreError> {
        let sidecar = Self::sidecar_path_for(photo_path);
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&sidecar, json)?;
        Ok(sidecar)
    }

    pub fn load_sidecar(photo_path: &Path) -> Result<Option<Self>, CoreError> {
        let sidecar = Self::sidecar_path_for(photo_path);
        if !sidecar.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(sidecar)?;
        let recipe: Self = serde_json::from_str(&text)?;
        Ok(Some(recipe))
    }
}

/// Interactive camera/viewport transform state for the central darkroom canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportCamera {
    /// Zoom multiplier relative to fit-to-viewport (`1.0` = Fit, `0.25..=16.0`).
    pub zoom: f32,
    /// Pan offset in normalized viewport units (`[-2.0, 2.0]`).
    pub pan: [f32; 2],
    /// View rotation in degrees (`-180.0..=180.0`).
    pub rotation_deg: f32,
    /// Whether Before/After comparison is enabled.
    pub split_compare: bool,
    /// Horizontal split divider position in `[0.0, 1.0]` (`0.5` = center).
    pub split_pos: f32,
    /// Momentary full-canvas "Before" preview (hold `\`).
    pub show_original: bool,
}

impl Default for ViewportCamera {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: [0.0, 0.0],
            rotation_deg: 0.0,
            split_compare: false,
            split_pos: 0.5,
            show_original: false,
        }
    }
}

impl ViewportCamera {
    pub fn reset(&mut self) {
        self.zoom = 1.0;
        self.pan = [0.0, 0.0];
        self.rotation_deg = 0.0;
    }

    pub fn zoom_by(&mut self, factor: f32) {
        self.zoom = (self.zoom * factor).clamp(0.25, 16.0);
        if (self.zoom - 1.0).abs() < 0.015 {
            self.zoom = 1.0;
            self.pan = [0.0, 0.0];
        }
    }

    pub fn pan_by(&mut self, dx_norm: f32, dy_norm: f32) {
        let limit = (self.zoom * 0.85).max(0.5);
        self.pan[0] = (self.pan[0] + dx_norm).clamp(-limit, limit);
        self.pan[1] = (self.pan[1] + dy_norm).clamp(-limit, limit);
    }
}

/// Curated built-in Capture One / Lightroom-inspired Styles.
#[derive(Clone, Debug)]
pub struct StylePreset {
    pub id: &'static str,
    pub name: &'static str,
    pub recipe: DevelopRecipe,
}

pub fn builtin_presets() -> Vec<StylePreset> {
    vec![
        StylePreset {
            id: "neutral",
            name: "Neutral",
            recipe: DevelopRecipe::default(),
        },
        StylePreset {
            id: "warm_film",
            name: "Warm Film",
            recipe: DevelopRecipe {
                exposure: 0.25,
                contrast: 0.18,
                highlights: -0.35,
                shadows: 0.28,
                whites: 0.10,
                blacks: -0.08,
                temperature: 0.26,
                tint: 0.06,
                vibrance: 0.22,
                saturation: -0.05,
                vignette: -0.18,
                grain: 0.14,
                preset_name: Some("Warm Film".to_owned()),
                ..DevelopRecipe::default()
            },
        },
        StylePreset {
            id: "classic_chrome",
            name: "Classic Chrome",
            recipe: DevelopRecipe {
                exposure: 0.05,
                contrast: 0.26,
                highlights: -0.42,
                shadows: -0.12,
                whites: 0.08,
                blacks: -0.15,
                temperature: -0.12,
                tint: -0.04,
                vibrance: -0.10,
                saturation: -0.22,
                vignette: -0.22,
                grain: 0.10,
                preset_name: Some("Classic Chrome".to_owned()),
                ..DevelopRecipe::default()
            },
        },
        StylePreset {
            id: "editorial_bw",
            name: "Editorial B&W",
            recipe: DevelopRecipe {
                exposure: 0.15,
                contrast: 0.42,
                highlights: -0.30,
                shadows: 0.15,
                whites: 0.22,
                blacks: -0.24,
                temperature: 0.08,
                tint: 0.0,
                vibrance: 0.0,
                saturation: -1.0,
                vignette: -0.32,
                grain: 0.24,
                preset_name: Some("Editorial B&W".to_owned()),
                ..DevelopRecipe::default()
            },
        },
        StylePreset {
            id: "moody_dusk",
            name: "Moody Dusk",
            recipe: DevelopRecipe {
                exposure: -0.20,
                contrast: 0.32,
                highlights: -0.55,
                shadows: 0.35,
                whites: 0.12,
                blacks: -0.20,
                temperature: -0.22,
                tint: 0.14,
                vibrance: 0.30,
                saturation: 0.05,
                vignette: -0.38,
                grain: 0.12,
                preset_name: Some("Moody Dusk".to_owned()),
                ..DevelopRecipe::default()
            },
        },
    ]
}

/// Bounded undo/redo history stack for non-destructive recipes.
#[derive(Clone, Debug)]
pub struct HistoryStack {
    past: Vec<DevelopRecipe>,
    future: Vec<DevelopRecipe>,
    max_len: usize,
}

impl Default for HistoryStack {
    fn default() -> Self {
        Self::new(128)
    }
}

impl HistoryStack {
    pub fn new(max_len: usize) -> Self {
        Self {
            past: Vec::new(),
            future: Vec::new(),
            max_len,
        }
    }

    pub fn push(&mut self, previous: DevelopRecipe) {
        if self.past.last() == Some(&previous) {
            return;
        }
        self.past.push(previous);
        if self.past.len() > self.max_len {
            self.past.remove(0);
        }
        self.future.clear();
    }

    pub fn undo(&mut self, current: &mut DevelopRecipe) -> bool {
        let Some(prev) = self.past.pop() else {
            return false;
        };
        let old = std::mem::replace(current, prev);
        self.future.push(old);
        true
    }

    pub fn redo(&mut self, current: &mut DevelopRecipe) -> bool {
        let Some(next) = self.future.pop() else {
            return false;
        };
        let old = std::mem::replace(current, next);
        self.past.push(old);
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    pub fn clear(&mut self) {
        self.past.clear();
        self.future.clear();
    }
}

/// 64-bin normalized RGB + Luminance histogram for the darkroom inspector.
pub const HISTOGRAM_BINS: usize = 64;

#[derive(Clone, Debug)]
pub struct HistogramData {
    pub r: [f32; HISTOGRAM_BINS],
    pub g: [f32; HISTOGRAM_BINS],
    pub b: [f32; HISTOGRAM_BINS],
    pub luma: [f32; HISTOGRAM_BINS],
    pub shadow_clip_pct: f32,
    pub highlight_clip_pct: f32,
}

impl Default for HistogramData {
    fn default() -> Self {
        Self {
            r: [0.0; HISTOGRAM_BINS],
            g: [0.0; HISTOGRAM_BINS],
            b: [0.0; HISTOGRAM_BINS],
            luma: [0.0; HISTOGRAM_BINS],
            shadow_clip_pct: 0.0,
            highlight_clip_pct: 0.0,
        }
    }
}

/// CPU reference implementation matching `develop.wgsl` so the inspector's live
/// histogram tracks the GPU viewport accurately on a fast downsampled linear proxy.
#[inline]
pub fn evaluate_pixel_linear(rgb: [f32; 3], recipe: &DevelopRecipe) -> [f32; 3] {
    // 1. White balance gains (smooth exponential warmth + green/magenta tint)
    let wb_r = 2.0_f32.powf(recipe.temperature * 0.45 + recipe.tint * 0.22);
    let wb_g = 2.0_f32.powf(-recipe.tint * 0.35);
    let wb_b = 2.0_f32.powf(-recipe.temperature * 0.45 + recipe.tint * 0.22);

    // 2. Exposure in linear light (2^EV)
    let exp_gain = 2.0_f32.powf(recipe.exposure);
    let mut r = (rgb[0] * wb_r * exp_gain).max(0.0);
    let mut g = (rgb[1] * wb_g * exp_gain).max(0.0);
    let mut b = (rgb[2] * wb_b * exp_gain).max(0.0);

    // 3. Tonal regions (Shadows / Highlights / Whites / Blacks) in perceptual log-like weight
    let luma = (0.2126 * r + 0.7152 * g + 0.0722 * b).max(1e-5);
    let t = (luma / (luma + 0.18)).clamp(0.0, 1.0); // 0.5 at 18% middle gray
    let shadow_w = (1.0 - t) * (1.0 - t);
    let highlight_w = t * t;
    let black_w = shadow_w * shadow_w;
    let white_w = highlight_w * highlight_w;

    let tone_stops = recipe.shadows * 1.15 * shadow_w
        + recipe.highlights * 1.15 * highlight_w
        + recipe.blacks * 0.90 * black_w
        + recipe.whites * 0.75 * white_w;
    let tone_gain = 2.0_f32.powf(tone_stops);
    r *= tone_gain;
    g *= tone_gain;
    b *= tone_gain;

    // 4. Contrast around 18% middle gray pivot
    let pivot = 0.18_f32;
    let c_pow = 2.0_f32.powf(recipe.contrast * 0.85);
    r = pivot * (r / pivot).max(0.0).powf(c_pow);
    g = pivot * (g / pivot).max(0.0).powf(c_pow);
    b = pivot * (b / pivot).max(0.0).powf(c_pow);

    // 5. Vibrance & Saturation
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let max_c = r.max(g).max(b);
    let min_c = r.min(g).min(b);
    let est_sat = if max_c > 1e-5 {
        ((max_c - min_c) / max_c).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let sat_scale = (1.0 + recipe.saturation + recipe.vibrance * (1.0 - est_sat) * 0.75).max(0.0);
    r = (y + (r - y) * sat_scale).max(0.0);
    g = (y + (g - y) * sat_scale).max(0.0);
    b = (y + (b - y) * sat_scale).max(0.0);

    // 6. Soft filmic highlight roll-off (keeps SDR crisp while preserving >1.0 HDR detail)
    [
        filmic_tonemap_channel(r),
        filmic_tonemap_channel(g),
        filmic_tonemap_channel(b),
    ]
}

#[inline]
fn filmic_tonemap_channel(x: f32) -> f32 {
    // Gentle shoulder starting above ~0.75 so midtones stay faithful and >1.0 HDR rolls off smoothly.
    let x = x.max(0.0);
    if x <= 0.75 {
        x
    } else {
        let over = x - 0.75;
        0.75 + (over / (1.0 + over * 4.0))
    }
}

#[inline]
pub fn linear_to_srgb(x: f32) -> f32 {
    let c = x.clamp(0.0, 1.0);
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

#[inline]
pub fn srgb_to_linear(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Computes a fast 64-bin RGB + Luma histogram from a downsampled linear RGB proxy.
pub fn compute_histogram(proxy_linear_rgb: &[[f32; 3]], recipe: &DevelopRecipe) -> HistogramData {
    let mut r_bins = [0u32; HISTOGRAM_BINS];
    let mut g_bins = [0u32; HISTOGRAM_BINS];
    let mut b_bins = [0u32; HISTOGRAM_BINS];
    let mut l_bins = [0u32; HISTOGRAM_BINS];
    let mut shadow_clips = 0u32;
    let mut highlight_clips = 0u32;

    let max_bin = (HISTOGRAM_BINS - 1) as f32;
    for &px in proxy_linear_rgb {
        let lin = evaluate_pixel_linear(px, recipe);
        let sr = linear_to_srgb(lin[0]);
        let sg = linear_to_srgb(lin[1]);
        let sb = linear_to_srgb(lin[2]);
        let sl = (0.2126 * sr + 0.7152 * sg + 0.0722 * sb).clamp(0.0, 1.0);

        if sr <= 0.004 && sg <= 0.004 && sb <= 0.004 {
            shadow_clips += 1;
        }
        if sr >= 0.992 || sg >= 0.992 || sb >= 0.992 {
            highlight_clips += 1;
        }

        r_bins[(sr * max_bin).round() as usize] += 1;
        g_bins[(sg * max_bin).round() as usize] += 1;
        b_bins[(sb * max_bin).round() as usize] += 1;
        l_bins[(sl * max_bin).round() as usize] += 1;
    }

    // Normalize against 98th percentile peak so extreme black/white borders don't crush midtones.
    let mut all_counts = Vec::with_capacity(HISTOGRAM_BINS * 3);
    all_counts.extend_from_slice(&r_bins[1..HISTOGRAM_BINS - 1]);
    all_counts.extend_from_slice(&g_bins[1..HISTOGRAM_BINS - 1]);
    all_counts.extend_from_slice(&b_bins[1..HISTOGRAM_BINS - 1]);
    all_counts.sort_unstable();
    let peak = all_counts
        .get((all_counts.len() * 98) / 100)
        .copied()
        .unwrap_or(1)
        .max(1) as f32;

    let mut out = HistogramData::default();
    for i in 0..HISTOGRAM_BINS {
        out.r[i] = (r_bins[i] as f32 / peak).min(1.0);
        out.g[i] = (g_bins[i] as f32 / peak).min(1.0);
        out.b[i] = (b_bins[i] as f32 / peak).min(1.0);
        out.luma[i] = (l_bins[i] as f32 / peak).min(1.0);
    }
    let total = proxy_linear_rgb.len().max(1) as f32;
    out.shadow_clip_pct = (shadow_clips as f32 / total) * 100.0;
    out.highlight_clip_pct = (highlight_clips as f32 / total) * 100.0;
    out
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_redo_roundtrips_recipe() {
        let mut history = HistoryStack::default();
        let mut recipe = DevelopRecipe::default();
        history.push(recipe.clone());
        recipe.exposure = 1.25;
        assert!(history.undo(&mut recipe));
        assert_eq!(recipe.exposure, 0.0);
        assert!(history.redo(&mut recipe));
        assert_eq!(recipe.exposure, 1.25);
    }

    #[test]
    fn exposure_doubles_linear_midtones() {
        let mut recipe = DevelopRecipe::default();
        let base = evaluate_pixel_linear([0.18, 0.18, 0.18], &recipe);
        recipe.exposure = 1.0;
        let plus_one = evaluate_pixel_linear([0.18, 0.18, 0.18], &recipe);
        assert!((plus_one[0] - base[0] * 2.0).abs() < 1e-4);
    }

    #[test]
    fn histogram_normalizes_cleanly() {
        let proxy = vec![[0.18, 0.18, 0.18]; 256];
        let hist = compute_histogram(&proxy, &DevelopRecipe::default());
        assert!(hist.luma.iter().copied().fold(0.0_f32, f32::max) > 0.5);
    }
}
