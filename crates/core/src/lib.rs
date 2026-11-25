//! UI- and GPU-agnostic core for Rune Editor.
//!
//! Owns the non-destructive photographic develop recipe, sidecar serialization,
//! Capture One 3-Way Color Balance, Fritsch-Carlson RGB/Luma Tone Curves,
//! 8-Band HSL Color Editor, session catalog & culling model, undo/redo history,
//! built-in styles, and scene-referred linear color math (including fast
//! precompiled evaluation for live histograms and full-resolution export).

pub mod curves;
pub mod session;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use crate::curves::{CURVE_LUT_SIZE, ToneCurves, default_curve_points, sample_curve_lut};
pub use crate::session::{CullFilter, CullFlag, PhotoMeta, SessionCatalog, SessionEntry};

/// Version tag embedded in `.rune-edit.json` sidecar files.
pub const RECIPE_SCHEMA_VERSION: u32 = 1;

/// Number of hue bands in the 8-Band Color Editor (HSL).
pub const HSL_BAND_COUNT: usize = 8;

/// Display names for the 8 HSL Color Editor bands.
pub const HSL_BAND_NAMES: [&str; HSL_BAND_COUNT] = [
    "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
];

/// Canonical hue center angles (degrees in `[0, 360)`) for the 8 HSL bands.
pub const HSL_BAND_HUES: [f32; HSL_BAND_COUNT] =
    [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 280.0, 320.0];

/// Capture One-style 3-Way Color Balance wheel control.
///
/// `(hue_x, hue_y)` lies inside the unit chroma disk (`hue_x^2 + hue_y^2 <= 1.0`),
/// where `+X` is Red (`0°`), `(-0.5, +0.866)` is Green (`120°`), and `(-0.5, -0.866)`
/// is Blue (`240°`). `luma` adjusts tonal lightness in `-1.0..=1.0`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ColorWheel {
    #[serde(default)]
    pub hue_x: f32,
    #[serde(default)]
    pub hue_y: f32,
    #[serde(default)]
    pub luma: f32,
}

impl ColorWheel {
    pub const fn new(hue_x: f32, hue_y: f32, luma: f32) -> Self {
        Self { hue_x, hue_y, luma }
    }

    pub fn is_neutral(&self) -> bool {
        self.hue_x == 0.0 && self.hue_y == 0.0 && self.luma == 0.0
    }

    /// Clamps `(hue_x, hue_y)` to the unit disk and `luma` to `-1.0..=1.0`.
    pub fn clamped(self) -> Self {
        let len = self.hue_x.hypot(self.hue_y);
        let (hx, hy) = if len > 1.0 {
            (self.hue_x / len, self.hue_y / len)
        } else {
            (self.hue_x, self.hue_y)
        };
        Self {
            hue_x: hx,
            hue_y: hy,
            luma: self.luma.clamp(-1.0, 1.0),
        }
    }

    /// Packs `[hue_x, hue_y, luma, 0.0]` for 16-byte aligned GPU uniforms.
    #[inline]
    pub fn to_uniform(&self) -> [f32; 4] {
        [self.hue_x, self.hue_y, self.luma, 0.0]
    }
}

/// Single band adjustment for the 8-Band HSL Color Editor.
///
/// Each component is normalized to `-1.0..=1.0`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HslBand {
    #[serde(default)]
    pub hue: f32,
    #[serde(default)]
    pub saturation: f32,
    #[serde(default)]
    pub luminance: f32,
}

impl HslBand {
    pub const fn new(hue: f32, saturation: f32, luminance: f32) -> Self {
        Self {
            hue,
            saturation,
            luminance,
        }
    }

    pub fn is_neutral(&self) -> bool {
        self.hue == 0.0 && self.saturation == 0.0 && self.luminance == 0.0
    }

    /// Packs `[hue, saturation, luminance, 0.0]` for 16-byte aligned GPU uniforms.
    #[inline]
    pub fn to_uniform(&self) -> [f32; 4] {
        [self.hue, self.saturation, self.luminance, 0.0]
    }
}

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
    /// Capture One 3-Way Color Balance — Shadows wheel.
    #[serde(default)]
    pub shadows_wheel: ColorWheel,
    /// Capture One 3-Way Color Balance — Midtones wheel.
    #[serde(default)]
    pub midtones_wheel: ColorWheel,
    /// Capture One 3-Way Color Balance — Highlights wheel.
    #[serde(default)]
    pub highlights_wheel: ColorWheel,
    /// RGB + Master Luma monotonic cubic Hermite tone curves.
    #[serde(default)]
    pub curves: ToneCurves,
    /// 8-Band HSL Color Editor (`Red`, `Orange`, `Yellow`, `Green`, `Aqua`, `Blue`, `Purple`, `Magenta`).
    #[serde(default)]
    pub hsl: [HslBand; HSL_BAND_COUNT],
    /// Local midtone contrast (`-1.0..=+1.0`).
    #[serde(default)]
    pub clarity: f32,
    /// Unsharp mask sharpening amount (`0.0..=1.0`).
    #[serde(default)]
    pub sharpen: f32,
    /// Edge-aware chroma/luma noise reduction (`0.0..=1.0`).
    #[serde(default)]
    pub noise_reduction: f32,
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
            shadows_wheel: ColorWheel::default(),
            midtones_wheel: ColorWheel::default(),
            highlights_wheel: ColorWheel::default(),
            curves: ToneCurves::default(),
            hsl: [HslBand::default(); HSL_BAND_COUNT],
            clarity: 0.0,
            sharpen: 0.0,
            noise_reduction: 0.0,
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
            && self.shadows_wheel.is_neutral()
            && self.midtones_wheel.is_neutral()
            && self.highlights_wheel.is_neutral()
            && self.curves.is_identity()
            && self.hsl.iter().all(HslBand::is_neutral)
            && self.clarity == 0.0
            && self.sharpen == 0.0
            && self.noise_reduction == 0.0
    }

    /// Compiles this recipe into a fast per-pixel evaluator with a pre-baked 256-entry
    /// tone curve LUT for high-throughput histograms and full-resolution exports.
    pub fn compile(&self) -> CompiledRecipe {
        CompiledRecipe::from_recipe(self)
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

    /// Loads a `DevelopRecipe` from a `.rune-edit.json` sidecar.
    ///
    /// Supports both bare `DevelopRecipe` JSON files and wrapped `PhotoMeta` sidecars.
    pub fn load_sidecar(photo_path: &Path) -> Result<Option<Self>, CoreError> {
        let sidecar = Self::sidecar_path_for(photo_path);
        if !sidecar.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(sidecar)?;
        let meta = PhotoMeta::from_sidecar_json(&text)?;
        Ok(Some(meta.recipe))
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
    let mut warm_hsl = [HslBand::default(); HSL_BAND_COUNT];
    warm_hsl[1] = HslBand::new(-0.08, 0.14, 0.06); // Warm golden oranges
    warm_hsl[2] = HslBand::new(-0.12, 0.08, 0.04); // Amber yellows
    warm_hsl[3] = HslBand::new(-0.10, -0.18, -0.06); // Olive film greens
    warm_hsl[5] = HslBand::new(-0.08, -0.15, -0.05); // Soft Kodak blues

    let mut chrome_hsl = [HslBand::default(); HSL_BAND_COUNT];
    chrome_hsl[0] = HslBand::new(0.05, 0.10, -0.06); // Deep documentary reds
    chrome_hsl[3] = HslBand::new(0.12, -0.28, -0.14); // Muted pine greens
    chrome_hsl[4] = HslBand::new(0.08, -0.26, -0.08); // Classic Chrome cyan skies
    chrome_hsl[5] = HslBand::new(-0.10, -0.32, -0.10); // Desaturated slate blues

    let mut bw_hsl = [HslBand::default(); HSL_BAND_COUNT];
    bw_hsl[0] = HslBand::new(0.0, 0.0, 0.12); // Red-filter skin & stone lift
    bw_hsl[1] = HslBand::new(0.0, 0.0, 0.18); // Warm highlights separation
    bw_hsl[4] = HslBand::new(0.0, 0.0, -0.24); // Deepen cyan skies
    bw_hsl[5] = HslBand::new(0.0, 0.0, -0.36); // Dramatic dark blue skies

    let mut dusk_hsl = [HslBand::default(); HSL_BAND_COUNT];
    dusk_hsl[1] = HslBand::new(-0.06, 0.20, 0.08); // Glowing cabin/horizon amber
    dusk_hsl[4] = HslBand::new(0.12, -0.10, -0.14); // Deep twilight aqua
    dusk_hsl[5] = HslBand::new(0.08, 0.12, -0.18); // Rich indigo dusk blues
    dusk_hsl[6] = HslBand::new(-0.08, 0.10, -0.10); // Subtle violet shadows

    let mut alpine_hsl = [HslBand::default(); HSL_BAND_COUNT];
    alpine_hsl[3] = HslBand::new(0.06, 0.08, -0.04); // Crisp evergreens
    alpine_hsl[4] = HslBand::new(0.04, 0.12, 0.04); // Glacial cyan
    alpine_hsl[5] = HslBand::new(-0.04, 0.14, -0.06); // Deep mountain sky

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
                exposure: 0.20,
                contrast: 0.14,
                highlights: -0.35,
                shadows: 0.25,
                whites: 0.08,
                blacks: -0.06,
                temperature: 0.22,
                tint: 0.05,
                vibrance: 0.18,
                saturation: -0.04,
                vignette: -0.18,
                grain: 0.14,
                preset_name: Some("Warm Film".to_owned()),
                shadows_wheel: ColorWheel::new(-0.14, -0.04, -0.02),
                midtones_wheel: ColorWheel::new(0.08, 0.04, 0.0),
                highlights_wheel: ColorWheel::new(0.24, 0.14, 0.03),
                curves: ToneCurves {
                    luma: vec![[0.0, 0.025], [0.25, 0.21], [0.75, 0.79], [1.0, 0.97]],
                    blue: vec![[0.0, 0.03], [0.5, 0.49], [1.0, 0.95]],
                    ..ToneCurves::default()
                },
                hsl: warm_hsl,
                clarity: 0.08,
                sharpen: 0.15,
                noise_reduction: 0.0,
                ..DevelopRecipe::default()
            },
        },
        StylePreset {
            id: "classic_chrome",
            name: "Classic Chrome",
            recipe: DevelopRecipe {
                exposure: 0.05,
                contrast: 0.22,
                highlights: -0.42,
                shadows: -0.10,
                whites: 0.08,
                blacks: -0.14,
                temperature: -0.10,
                tint: -0.03,
                vibrance: -0.08,
                saturation: -0.16,
                vignette: -0.22,
                grain: 0.10,
                preset_name: Some("Classic Chrome".to_owned()),
                shadows_wheel: ColorWheel::new(-0.18, -0.06, -0.04),
                midtones_wheel: ColorWheel::new(-0.04, 0.02, 0.0),
                highlights_wheel: ColorWheel::new(0.10, 0.05, -0.02),
                curves: ToneCurves {
                    luma: vec![[0.0, 0.0], [0.22, 0.17], [0.78, 0.82], [1.0, 0.98]],
                    ..ToneCurves::default()
                },
                hsl: chrome_hsl,
                clarity: 0.16,
                sharpen: 0.22,
                noise_reduction: 0.0,
                ..DevelopRecipe::default()
            },
        },
        StylePreset {
            id: "editorial_bw",
            name: "Editorial B&W",
            recipe: DevelopRecipe {
                exposure: 0.14,
                contrast: 0.36,
                highlights: -0.30,
                shadows: 0.14,
                whites: 0.20,
                blacks: -0.22,
                temperature: 0.06,
                tint: 0.0,
                vibrance: 0.0,
                saturation: -1.0,
                vignette: -0.32,
                grain: 0.24,
                preset_name: Some("Editorial B&W".to_owned()),
                curves: ToneCurves {
                    luma: vec![
                        [0.0, 0.0],
                        [0.20, 0.12],
                        [0.50, 0.50],
                        [0.80, 0.88],
                        [1.0, 1.0],
                    ],
                    ..ToneCurves::default()
                },
                hsl: bw_hsl,
                clarity: 0.24,
                sharpen: 0.25,
                noise_reduction: 0.0,
                ..DevelopRecipe::default()
            },
        },
        StylePreset {
            id: "moody_dusk",
            name: "Moody Dusk",
            recipe: DevelopRecipe {
                exposure: -0.18,
                contrast: 0.26,
                highlights: -0.52,
                shadows: 0.32,
                whites: 0.10,
                blacks: -0.16,
                temperature: -0.18,
                tint: 0.10,
                vibrance: 0.26,
                saturation: 0.04,
                vignette: -0.36,
                grain: 0.12,
                preset_name: Some("Moody Dusk".to_owned()),
                shadows_wheel: ColorWheel::new(-0.16, -0.26, -0.05),
                midtones_wheel: ColorWheel::new(-0.06, -0.04, 0.0),
                highlights_wheel: ColorWheel::new(0.28, 0.12, 0.02),
                curves: ToneCurves {
                    luma: vec![[0.0, 0.04], [0.25, 0.22], [0.72, 0.76], [1.0, 0.95]],
                    ..ToneCurves::default()
                },
                hsl: dusk_hsl,
                clarity: 0.14,
                sharpen: 0.14,
                noise_reduction: 0.12,
                ..DevelopRecipe::default()
            },
        },
        StylePreset {
            id: "alpine_crisp",
            name: "Alpine Crisp",
            recipe: DevelopRecipe {
                exposure: 0.10,
                contrast: 0.18,
                highlights: -0.38,
                shadows: 0.22,
                whites: 0.14,
                blacks: -0.10,
                temperature: -0.06,
                tint: 0.02,
                vibrance: 0.20,
                saturation: 0.04,
                vignette: -0.12,
                grain: 0.0,
                preset_name: Some("Alpine Crisp".to_owned()),
                shadows_wheel: ColorWheel::new(-0.10, -0.12, 0.0),
                midtones_wheel: ColorWheel::new(0.02, 0.0, 0.02),
                highlights_wheel: ColorWheel::new(0.12, 0.08, 0.02),
                curves: ToneCurves {
                    luma: vec![[0.0, 0.0], [0.25, 0.23], [0.75, 0.78], [1.0, 1.0]],
                    ..ToneCurves::default()
                },
                hsl: alpine_hsl,
                clarity: 0.22,
                sharpen: 0.30,
                noise_reduction: 0.08,
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

/// Precompiled develop recipe holding baked 256-entry tone curve LUT and precomputed
/// scalar gains for fast per-pixel evaluation during live histogram and full-res export.
#[derive(Clone, Debug)]
pub struct CompiledRecipe {
    pub wb_exp: [f32; 3],
    pub contrast_pow: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub vibrance: f32,
    pub saturation: f32,
    pub shadows_wheel: ColorWheel,
    pub midtones_wheel: ColorWheel,
    pub highlights_wheel: ColorWheel,
    pub wheels_active: bool,
    pub curve_lut: [[f32; 4]; CURVE_LUT_SIZE],
    pub curves_active: bool,
    pub hsl: [HslBand; HSL_BAND_COUNT],
    pub hsl_active: bool,
}

impl CompiledRecipe {
    pub fn from_recipe(recipe: &DevelopRecipe) -> Self {
        let wb_r = 2.0_f32.powf(recipe.temperature * 0.45 + recipe.tint * 0.22);
        let wb_g = 2.0_f32.powf(-recipe.tint * 0.35);
        let wb_b = 2.0_f32.powf(-recipe.temperature * 0.45 + recipe.tint * 0.22);
        let exp_gain = 2.0_f32.powf(recipe.exposure);

        let shadows_wheel = recipe.shadows_wheel.clamped();
        let midtones_wheel = recipe.midtones_wheel.clamped();
        let highlights_wheel = recipe.highlights_wheel.clamped();
        let wheels_active = !shadows_wheel.is_neutral()
            || !midtones_wheel.is_neutral()
            || !highlights_wheel.is_neutral();

        let curves_active = !recipe.curves.is_identity();
        let curve_lut = recipe.curves.bake_curve_lut_f32();
        let hsl_active = recipe.hsl.iter().any(|b| !b.is_neutral());

        Self {
            wb_exp: [wb_r * exp_gain, wb_g * exp_gain, wb_b * exp_gain],
            contrast_pow: 2.0_f32.powf(recipe.contrast * 0.85),
            highlights: recipe.highlights,
            shadows: recipe.shadows,
            whites: recipe.whites,
            blacks: recipe.blacks,
            vibrance: recipe.vibrance,
            saturation: recipe.saturation,
            shadows_wheel,
            midtones_wheel,
            highlights_wheel,
            wheels_active,
            curve_lut,
            curves_active,
            hsl: recipe.hsl,
            hsl_active,
        }
    }

    /// Evaluates stages 1–7 in scene-referred linear light *before* optical vignette,
    /// film grain, and filmic highlight roll-off (matching `shaders/develop.wgsl` stages 1–7).
    ///
    /// Use this in full-resolution export when applying vignette/grain pre-tonemap, then
    /// finish with [`filmic_tonemap`].
    #[inline]
    pub fn evaluate_pixel_scene_linear(&self, rgb: [f32; 3]) -> [f32; 3] {
        // 1. White balance + Exposure (linear stops)
        let mut r = (rgb[0] * self.wb_exp[0]).max(0.0);
        let mut g = (rgb[1] * self.wb_exp[1]).max(0.0);
        let mut b = (rgb[2] * self.wb_exp[2]).max(0.0);

        // 2. Tonal regions (Shadows / Highlights / Whites / Blacks)
        let luma = (0.2126 * r + 0.7152 * g + 0.0722 * b).max(1e-5);
        let t = (luma / (luma + 0.18)).clamp(0.0, 1.0);
        let shadow_w = (1.0 - t) * (1.0 - t);
        let highlight_w = t * t;
        let black_w = shadow_w * shadow_w;
        let white_w = highlight_w * highlight_w;

        let tone_stops = self.shadows * 1.15 * shadow_w
            + self.highlights * 1.15 * highlight_w
            + self.blacks * 0.90 * black_w
            + self.whites * 0.75 * white_w;
        let tone_gain = 2.0_f32.powf(tone_stops);
        r *= tone_gain;
        g *= tone_gain;
        b *= tone_gain;

        // 3. Contrast around 18% middle gray pivot
        let pivot = 0.18_f32;
        r = pivot * (r / pivot).max(0.0).powf(self.contrast_pow);
        g = pivot * (g / pivot).max(0.0).powf(self.contrast_pow);
        b = pivot * (b / pivot).max(0.0).powf(self.contrast_pow);

        // 4. Tone Curves (Luma + per-channel R/G/B in perceptual gamma 2.2 space, HDR-preserving)
        if self.curves_active {
            let [cr, cg, cb] = apply_tone_curves_lut([r, g, b], &self.curve_lut);
            r = cr;
            g = cg;
            b = cb;
        }

        // 5. Capture One 3-Way Color Balance (Shadows / Midtones / Highlights wheels)
        if self.wheels_active {
            let [wr, wg, wb] = apply_color_wheels(
                [r, g, b],
                self.shadows_wheel,
                self.midtones_wheel,
                self.highlights_wheel,
            );
            r = wr;
            g = wg;
            b = wb;
        }

        // 6. 8-Band HSL Color Editor
        if self.hsl_active {
            let [hr, hg, hb] = apply_hsl_bands([r, g, b], &self.hsl);
            r = hr;
            g = hg;
            b = hb;
        }

        // 7. Vibrance & Saturation
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let max_c = r.max(g).max(b);
        let min_c = r.min(g).min(b);
        let est_sat = if max_c > 1e-5 {
            ((max_c - min_c) / max_c).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let sat_scale = (1.0 + self.saturation + self.vibrance * (1.0 - est_sat) * 0.75).max(0.0);
        r = (y + (r - y) * sat_scale).max(0.0);
        g = (y + (g - y) * sat_scale).max(0.0);
        b = (y + (b - y) * sat_scale).max(0.0);

        [r, g, b]
    }

    /// Evaluates a single scene-referred linear RGB pixel, matching `shaders/develop.wgsl`
    /// across WB, Exposure, Tones, Contrast, Tone Curves, 3-Way Color Balance,
    /// 8-Band HSL, Vibrance/Saturation, and Filmic tone mapping.
    #[inline]
    pub fn evaluate_pixel_linear(&self, rgb: [f32; 3]) -> [f32; 3] {
        filmic_tonemap(self.evaluate_pixel_scene_linear(rgb))
    }
}

/// CPU reference implementation matching `develop.wgsl` so the inspector's live
/// histogram and full-resolution export match the GPU viewport accurately.
///
/// For batch pixel evaluation (histograms, full-res exports), prefer
/// `CompiledRecipe::from_recipe(recipe)` so the 256-entry tone curve LUT is baked once.
#[inline]
pub fn evaluate_pixel_linear(rgb: [f32; 3], recipe: &DevelopRecipe) -> [f32; 3] {
    CompiledRecipe::from_recipe(recipe).evaluate_pixel_linear(rgb)
}

#[inline]
fn apply_tone_curves_lut(rgb: [f32; 3], lut: &[[f32; 4]; CURVE_LUT_SIZE]) -> [f32; 3] {
    const INV_GAMMA: f32 = 1.0 / 2.2;
    const GAMMA: f32 = 2.2;

    let pr = rgb[0].max(0.0).powf(INV_GAMMA);
    let pg = rgb[1].max(0.0).powf(INV_GAMMA);
    let pb = rgb[2].max(0.0).powf(INV_GAMMA);

    // Master Luma curve (LUT alpha channel) with HDR headroom preservation
    let lr = sample_curve_lut(lut, pr.min(1.0))[3] + (pr - 1.0).max(0.0);
    let lg = sample_curve_lut(lut, pg.min(1.0))[3] + (pg - 1.0).max(0.0);
    let lb = sample_curve_lut(lut, pb.min(1.0))[3] + (pb - 1.0).max(0.0);

    // Per-channel R / G / B curves (LUT r/g/b channels) with HDR headroom preservation
    let or = sample_curve_lut(lut, lr.min(1.0))[0] + (lr - 1.0).max(0.0);
    let og = sample_curve_lut(lut, lg.min(1.0))[1] + (lg - 1.0).max(0.0);
    let ob = sample_curve_lut(lut, lb.min(1.0))[2] + (lb - 1.0).max(0.0);

    [
        or.max(0.0).powf(GAMMA),
        og.max(0.0).powf(GAMMA),
        ob.max(0.0).powf(GAMMA),
    ]
}

#[inline]
fn wheel_tint_rgb(wheel: ColorWheel) -> [f32; 3] {
    const SQRT3_OVER_2: f32 = 0.866_025_4;
    let tr = wheel.hue_x;
    let tg = -0.5 * wheel.hue_x + SQRT3_OVER_2 * wheel.hue_y;
    let tb = -0.5 * wheel.hue_x - SQRT3_OVER_2 * wheel.hue_y;
    // Subtract Rec.709 luminance so the hue wheel is luminance-neutral.
    let y = 0.2126 * tr + 0.7152 * tg + 0.0722 * tb;
    [tr - y, tg - y, tb - y]
}

#[inline]
fn apply_color_wheels(
    rgb: [f32; 3],
    shadows: ColorWheel,
    midtones: ColorWheel,
    highlights: ColorWheel,
) -> [f32; 3] {
    let luma = (0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]).max(1e-5);
    let t = (luma / (luma + 0.18)).clamp(0.0, 1.0);
    let w_s = (1.0 - t) * (1.0 - t);
    let w_m = 2.0 * t * (1.0 - t);
    let w_h = t * t;

    let ts = wheel_tint_rgb(shadows);
    let tm = wheel_tint_rgb(midtones);
    let th = wheel_tint_rgb(highlights);

    let luma_shift = shadows.luma * w_s + midtones.luma * w_m + highlights.luma * w_h;
    let shadow_lift = w_s * w_s * 0.025;

    let mut out = [0.0_f32; 3];
    for i in 0..3 {
        let color_shift = ts[i] * w_s + tm[i] * w_m + th[i] * w_h;
        let gain = 2.0_f32.powf(color_shift * 0.65 + luma_shift * 0.75);
        out[i] = (rgb[i] * gain + ts[i] * shadow_lift).max(0.0);
    }
    out
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn rgb_to_hsv(rgb: [f32; 3]) -> [f32; 3] {
    let r = rgb[0];
    let g = rgb[1];
    let b = rgb[2];
    let max_c = r.max(g).max(b);
    let min_c = r.min(g).min(b);
    let delta = max_c - min_c;

    let s = if max_c > 1e-6 {
        (delta / max_c).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let h = if delta <= 1e-6 {
        0.0
    } else if max_c == r {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if max_c == g {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };

    [h.rem_euclid(360.0), s, max_c]
}

#[inline]
fn hsv_to_rgb(hsv: [f32; 3]) -> [f32; 3] {
    let h = hsv[0].rem_euclid(360.0);
    let s = hsv[1].clamp(0.0, 1.0);
    let v = hsv[2].max(0.0);

    let c = v * s;
    let sector = h / 60.0;
    let x = c * (1.0 - ((sector % 2.0) - 1.0).abs());
    let m = v - c;

    let (r1, g1, b1) = if sector < 1.0 {
        (c, x, 0.0)
    } else if sector < 2.0 {
        (x, c, 0.0)
    } else if sector < 3.0 {
        (0.0, c, x)
    } else if sector < 4.0 {
        (0.0, x, c)
    } else if sector < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    [r1 + m, g1 + m, b1 + m]
}

#[inline]
fn apply_hsl_bands(rgb: [f32; 3], bands: &[HslBand; HSL_BAND_COUNT]) -> [f32; 3] {
    const INV_GAMMA: f32 = 1.0 / 2.2;
    const GAMMA: f32 = 2.2;

    let p = [
        rgb[0].max(0.0).powf(INV_GAMMA),
        rgb[1].max(0.0).powf(INV_GAMMA),
        rgb[2].max(0.0).powf(INV_GAMMA),
    ];
    let [h_deg, s, v] = rgb_to_hsv(p);
    let chroma_mask = smoothstep(0.02, 0.14, s);
    if chroma_mask <= 0.0 {
        return rgb;
    }

    // Locate adjacent hue band centers around the 360° circle for C1-smooth partition of unity.
    let mut i0 = HSL_BAND_COUNT - 1;
    let mut i1 = 0usize;
    let mut h0 = HSL_BAND_HUES[HSL_BAND_COUNT - 1];
    let mut h1 = 360.0_f32;
    let mut h_eval = h_deg;
    if h_deg < HSL_BAND_HUES[0] {
        h_eval += 360.0;
    } else {
        for i in 0..HSL_BAND_COUNT - 1 {
            if h_deg >= HSL_BAND_HUES[i] && h_deg < HSL_BAND_HUES[i + 1] {
                i0 = i;
                i1 = i + 1;
                h0 = HSL_BAND_HUES[i];
                h1 = HSL_BAND_HUES[i + 1];
                break;
            }
        }
    }

    let frac = ((h_eval - h0) / (h1 - h0)).clamp(0.0, 1.0);
    let w1 = frac * frac * (3.0 - 2.0 * frac);
    let w0 = 1.0 - w1;

    let b0 = bands[i0];
    let b1 = bands[i1];
    let hue_adj = b0.hue * w0 + b1.hue * w1;
    let sat_adj = b0.saturation * w0 + b1.saturation * w1;
    let lum_adj = b0.luminance * w0 + b1.luminance * w1;

    if hue_adj.abs() + sat_adj.abs() + lum_adj.abs() <= 1e-6 {
        return rgb;
    }

    let h_new = (h_deg + hue_adj * 30.0 * chroma_mask).rem_euclid(360.0);
    let s_new = (s * (1.0 + sat_adj * chroma_mask)).clamp(0.0, 1.0);
    let v_new = v * 2.0_f32.powf(lum_adj * 0.65 * chroma_mask);

    let p_out = hsv_to_rgb([h_new, s_new, v_new]);
    [
        p_out[0].max(0.0).powf(GAMMA),
        p_out[1].max(0.0).powf(GAMMA),
        p_out[2].max(0.0).powf(GAMMA),
    ]
}

/// Soft filmic highlight roll-off matching `filmic_tonemap` in `shaders/develop.wgsl`.
///
/// Leaves midtones (`<= 0.75` linear) untouched and smoothly compresses `> 0.75` HDR highlights
/// into `(0.75, 1.0)`.
#[inline]
pub fn filmic_tonemap(rgb: [f32; 3]) -> [f32; 3] {
    [
        filmic_tonemap_channel(rgb[0]),
        filmic_tonemap_channel(rgb[1]),
        filmic_tonemap_channel(rgb[2]),
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
    let compiled = CompiledRecipe::from_recipe(recipe);
    let mut r_bins = [0u32; HISTOGRAM_BINS];
    let mut g_bins = [0u32; HISTOGRAM_BINS];
    let mut b_bins = [0u32; HISTOGRAM_BINS];
    let mut l_bins = [0u32; HISTOGRAM_BINS];
    let mut shadow_clips = 0u32;
    let mut highlight_clips = 0u32;

    let max_bin = (HISTOGRAM_BINS - 1) as f32;
    for &px in proxy_linear_rgb {
        let lin = compiled.evaluate_pixel_linear(px);
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

    #[test]
    fn curve_identity_and_monotonicity() {
        let id_curves = ToneCurves::default();
        assert!(id_curves.is_identity());
        let id_lut = id_curves.bake_curve_lut_f32();
        for (i, entry) in id_lut.iter().enumerate() {
            let expected = i as f32 / 255.0;
            for (ch, &val) in entry.iter().enumerate() {
                assert!(
                    (val - expected).abs() < 1e-6,
                    "identity LUT mismatch at {i} ch {ch}: got {val}, expected {expected}",
                );
            }
        }

        // Steep monotonic S-curve must never overshoot or become non-monotonic.
        let steep = vec![
            [0.0, 0.0],
            [0.35, 0.05],
            [0.50, 0.50],
            [0.65, 0.95],
            [1.0, 1.0],
        ];
        let curves = ToneCurves {
            luma: steep.clone(),
            red: vec![[0.0, 0.05], [0.25, 0.18], [0.75, 0.84], [1.0, 0.96]],
            ..ToneCurves::default()
        };
        assert!(!curves.is_identity());
        let lut = curves.bake_curve_lut_f32();
        for i in 1..CURVE_LUT_SIZE {
            assert!(
                lut[i][3] + 1e-6 >= lut[i - 1][3],
                "luma curve non-monotonic at {i}: {} < {}",
                lut[i][3],
                lut[i - 1][3]
            );
            assert!(
                lut[i][0] + 1e-6 >= lut[i - 1][0],
                "red curve non-monotonic at {i}: {} < {}",
                lut[i][0],
                lut[i - 1][0]
            );
            for &val in &lut[i] {
                assert!((0.0..=1.0).contains(&val));
            }
        }

        // Exact knot interpolation.
        for &[kx, ky] in &steep {
            let v = ToneCurves::evaluate_channel(&steep, kx);
            assert!((v - ky).abs() < 1e-5, "knot ({kx}, {ky}) evaluated to {v}");
        }

        // Unsorted / duplicate / out-of-range knots must sanitize cleanly.
        let messy = vec![[1.2, 1.4], [0.5, 0.6], [0.5, 0.65], [-0.2, -0.1]];
        let v = ToneCurves::evaluate_channel(&messy, 0.5);
        assert!(v.is_finite() && (0.0..=1.0).contains(&v));
    }

    #[test]
    fn three_way_color_balance_tonal_selectivity() {
        let shadow_px = [0.025, 0.025, 0.025];
        let mid_px = [0.18, 0.18, 0.18];
        let hi_px = [0.65, 0.65, 0.65];

        // Warm highlights wheel shifts highlights warmer with minimal shadow impact.
        let warm_hi = DevelopRecipe {
            highlights_wheel: ColorWheel::new(0.8, 0.25, 0.0),
            ..DevelopRecipe::default()
        };
        let s_out = evaluate_pixel_linear(shadow_px, &warm_hi);
        let h_out = evaluate_pixel_linear(hi_px, &warm_hi);
        let h_warmth = (h_out[0] - h_out[2]) / hi_px[0];
        let s_warmth = (s_out[0] - s_out[2]) / shadow_px[0];
        assert!(
            h_warmth > 0.15,
            "highlights should shift warm, got {h_warmth}"
        );
        assert!(
            h_warmth > s_warmth * 4.0,
            "highlights wheel should selectively target highlights ({h_warmth}) over shadows ({s_warmth})"
        );

        // Cool blue shadows wheel shifts shadows cool blue with minimal highlight impact.
        let cool_sh = DevelopRecipe {
            shadows_wheel: ColorWheel::new(-0.4, -0.75, 0.0),
            ..DevelopRecipe::default()
        };
        let s_out = evaluate_pixel_linear(shadow_px, &cool_sh);
        let h_out = evaluate_pixel_linear(hi_px, &cool_sh);
        let s_cool = (s_out[2] - s_out[0]) / shadow_px[0];
        let h_cool = (h_out[2] - h_out[0]) / hi_px[0];
        assert!(
            s_cool > 0.25,
            "shadows should shift cool blue, got {s_cool}"
        );
        assert!(
            s_cool > h_cool * 4.0,
            "shadows wheel should selectively target shadows ({s_cool}) over highlights ({h_cool})"
        );

        // Midtones luma wheel lifts middle gray cleanly.
        let mid_lift = DevelopRecipe {
            midtones_wheel: ColorWheel::new(0.0, 0.0, 0.8),
            ..DevelopRecipe::default()
        };
        let m_base = evaluate_pixel_linear(mid_px, &DevelopRecipe::default());
        let m_lifted = evaluate_pixel_linear(mid_px, &mid_lift);
        assert!(m_lifted[1] > m_base[1] * 1.15);
    }

    #[test]
    fn hsl_band_selectivity_and_neutral_protection() {
        let blue_px = [0.02, 0.02, 0.45];
        let red_px = [0.45, 0.02, 0.02];
        let green_px = [0.02, 0.45, 0.02];
        let gray_px = [0.18, 0.18, 0.18];

        let mut recipe = DevelopRecipe::default();
        recipe.hsl[5] = HslBand::new(0.0, -1.0, 0.0); // Fully desaturate Blue band

        let blue_base = evaluate_pixel_linear(blue_px, &DevelopRecipe::default());
        let blue_desat = evaluate_pixel_linear(blue_px, &recipe);
        assert!(
            (blue_desat[0] - blue_desat[2]).abs() < 1e-4,
            "blue pixel should be fully desaturated: {blue_desat:?} (was {blue_base:?})"
        );

        // Red, Green, and neutral Gray must remain untouched by the Blue band.
        for px in [red_px, green_px, gray_px] {
            let base = evaluate_pixel_linear(px, &DevelopRecipe::default());
            let after = evaluate_pixel_linear(px, &recipe);
            for ch in 0..3 {
                assert!(
                    (base[ch] - after[ch]).abs() < 1e-5,
                    "pixel {px:?} unexpectedly shifted by Blue HSL band: {base:?} -> {after:?}"
                );
            }
        }

        // Red luminance shift brightens Red without affecting Gray or Blue.
        let mut red_lum = DevelopRecipe::default();
        red_lum.hsl[0] = HslBand::new(0.0, 0.0, 0.8);
        let red_before = evaluate_pixel_linear(red_px, &DevelopRecipe::default());
        let red_after = evaluate_pixel_linear(red_px, &red_lum);
        assert!(red_after[0] > red_before[0] * 1.2);
        let gray_after = evaluate_pixel_linear(gray_px, &red_lum);
        assert!((gray_after[0] - gray_px[0]).abs() < 1e-6);
    }

    #[test]
    fn presets_neutral_identity_and_compiled_parity() {
        let presets = builtin_presets();
        assert!(presets.len() >= 5);
        let neutral = presets
            .iter()
            .find(|p| p.id == "neutral")
            .expect("Neutral preset");
        assert!(neutral.recipe.is_neutral());

        let samples = [
            [0.01, 0.02, 0.04],
            [0.18, 0.18, 0.18],
            [0.60, 0.25, 0.08],
            [0.05, 0.35, 0.80],
            [1.60, 1.10, 0.50],
        ];
        for preset in &presets {
            let compiled = preset.recipe.compile();
            for &px in &samples {
                let a = evaluate_pixel_linear(px, &preset.recipe);
                let b = compiled.evaluate_pixel_linear(px);
                for ch in 0..3 {
                    assert!(a[ch].is_finite() && a[ch] >= 0.0);
                    assert!((a[ch] - b[ch]).abs() < 1e-7);
                }
            }
        }
    }

    #[test]
    fn session_catalog_filtering_navigation_and_copy_paste() {
        let paths = vec![
            PathBuf::from("/tmp/rune_test_nonexistent_a.dng"),
            PathBuf::from("/tmp/rune_test_nonexistent_b.dng"),
            PathBuf::from("/tmp/rune_test_nonexistent_c.dng"),
            PathBuf::from("/tmp/rune_test_nonexistent_d.dng"),
        ];
        let mut catalog = SessionCatalog::from_paths(Some(PathBuf::from("/tmp")), paths);
        assert_eq!(catalog.len(), 4);
        assert_eq!(catalog.active_index, Some(0));

        catalog.set_rating(0, 5);
        catalog.set_flag(0, CullFlag::Pick);
        catalog.set_rating(1, 2);
        catalog.set_flag(1, CullFlag::Reject);
        catalog.set_rating(2, 4);
        catalog.set_flag(2, CullFlag::Pick);
        catalog.set_rating(3, 9); // Clamps to 5
        assert_eq!(catalog.entries[3].meta.rating, 5);
        assert_eq!(catalog.counts(), (4, 2, 1));

        // Filter PicksOnly -> indices [0, 2]
        catalog.set_filter(CullFilter::PicksOnly);
        assert_eq!(catalog.filtered_indices(), vec![0, 2]);
        assert_eq!(catalog.active_index, Some(0));
        assert_eq!(catalog.select_next(), Some(2));
        assert_eq!(catalog.select_next(), None);
        assert_eq!(catalog.neighbor_paths().len(), 1);
        assert_eq!(catalog.select_prev(), Some(0));

        // Filter MinRating(4) -> indices [0, 2, 3]
        catalog.set_filter(CullFilter::MinRating(4));
        assert_eq!(catalog.filtered_indices(), vec![0, 2, 3]);

        // Copy / Paste Adjustments across photos
        let mut custom = DevelopRecipe {
            exposure: 0.75,
            clarity: 0.35,
            shadows_wheel: ColorWheel::new(-0.2, -0.3, -0.05),
            ..DevelopRecipe::default()
        };
        custom.hsl[5] = HslBand::new(0.1, -0.25, -0.15);
        catalog.update_active_recipe(custom.clone());
        assert!(catalog.copy_active_adjustments());
        assert!(catalog.has_clipboard());

        catalog.select_index(2);
        let pasted = catalog.paste_to_active().expect("clipboard should paste");
        assert_eq!(pasted, custom);
        assert_eq!(catalog.active_meta().unwrap().recipe, custom);
    }

    #[test]
    fn sidecar_backward_compatibility_and_roundtrip() {
        // Legacy v1 sidecar JSON without any of the new color balance / curve / HSL / detail fields.
        let legacy_json = r#"{
            "schema_version": 1,
            "exposure": 0.65,
            "contrast": 0.2,
            "highlights": -0.4,
            "shadows": 0.3,
            "whites": 0.1,
            "blacks": -0.1,
            "temperature": 0.15,
            "tint": -0.05,
            "vibrance": 0.25,
            "saturation": -0.1,
            "vignette": -0.2,
            "grain": 0.1,
            "preset_name": null
        }"#;

        let parsed_recipe: DevelopRecipe =
            serde_json::from_str(legacy_json).expect("legacy recipe must deserialize");
        assert!((parsed_recipe.exposure - 0.65).abs() < 1e-6);
        assert!(parsed_recipe.shadows_wheel.is_neutral());
        assert!(parsed_recipe.curves.is_identity());
        assert!(parsed_recipe.hsl.iter().all(HslBand::is_neutral));
        assert_eq!(parsed_recipe.clarity, 0.0);
        assert_eq!(parsed_recipe.sharpen, 0.0);
        assert_eq!(parsed_recipe.noise_reduction, 0.0);

        let parsed_meta = PhotoMeta::from_sidecar_json(legacy_json)
            .expect("PhotoMeta must load legacy bare DevelopRecipe sidecar");
        assert_eq!(parsed_meta.rating, 0);
        assert_eq!(parsed_meta.flag, CullFlag::None);
        assert_eq!(parsed_meta.recipe, parsed_recipe);

        // Full PhotoMeta sidecar disk roundtrip in a unique temp path
        let tmp_photo =
            std::env::temp_dir().join(format!("rune_core_sidecar_test_{}.dng", std::process::id()));
        let mut meta = PhotoMeta {
            rating: 4,
            flag: CullFlag::Pick,
            recipe: DevelopRecipe {
                exposure: -0.35,
                shadows_wheel: ColorWheel::new(-0.15, -0.25, -0.04),
                highlights_wheel: ColorWheel::new(0.22, 0.10, 0.02),
                curves: ToneCurves {
                    luma: vec![[0.0, 0.03], [0.5, 0.52], [1.0, 0.96]],
                    ..ToneCurves::default()
                },
                clarity: 0.28,
                sharpen: 0.40,
                noise_reduction: 0.18,
                ..DevelopRecipe::default()
            },
        };
        meta.recipe.hsl[1] = HslBand::new(-0.1, 0.2, 0.05);

        let sidecar_path = meta.save_sidecar(&tmp_photo).expect("save sidecar");
        let loaded_meta = PhotoMeta::load_sidecar(&tmp_photo)
            .expect("load PhotoMeta sidecar")
            .expect("sidecar should exist");
        let loaded_recipe = DevelopRecipe::load_sidecar(&tmp_photo)
            .expect("DevelopRecipe::load_sidecar should read PhotoMeta sidecars")
            .expect("recipe should exist");

        let _ = std::fs::remove_file(&sidecar_path);
        assert_eq!(loaded_meta, meta);
        assert_eq!(loaded_recipe, meta.recipe);
    }

    #[test]
    fn scene_linear_preserves_hdr_before_filmic_tonemap() {
        let recipe = DevelopRecipe {
            exposure: 1.0,
            ..DevelopRecipe::default()
        };
        let compiled = recipe.compile();
        let hdr_px = [1.5, 1.0, 0.4];
        let scene = compiled.evaluate_pixel_scene_linear(hdr_px);
        assert!(
            scene[0] > 2.5,
            "pre-tonemap scene linear should preserve >1.0 HDR, got {scene:?}"
        );
        let mapped = filmic_tonemap(scene);
        assert!(mapped[0] < 1.0 && mapped[0] > 0.75);
        assert_eq!(mapped, compiled.evaluate_pixel_linear(hdr_px));
    }
}
