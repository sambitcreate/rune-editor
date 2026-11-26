//! Typed sparse `RecipePatch`, composable `EditOp` pipelines, sidecar provenance,
//! and declarative `BatchJob` manifest structures.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::{
    ADVANCED_COLOR_SLICE_COUNT, ClarityMethod, CoreError, DevelopRecipe, FilmCurve,
    FilmNegativeParams, GrainType, HSL_BAND_COUNT, ProcessingMode, VignetteMethod, WbPreset,
    builtin_presets,
    geometry::{CompositionGrid, CropAspect},
    registry::{SCALAR_PARAMS, recipe_field_diffs},
    session::{ColorTag, CullFlag},
};

const fn default_true() -> bool {
    true
}

const fn default_one() -> f32 {
    1.0
}

/// Scalar mutation accepting bare numbers (`0.35`), signed relative strings (`"+0.25"`, `"-0.10"`),
/// or explicit objects (`{"set": 0.35}`, `{"add": 0.25}`, `{"scale": 1.2}`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScalarOp {
    Set(f32),
    Add(f32),
    Scale(f32),
}

impl ScalarOp {
    pub fn apply(self, current: f32, min: f32, max: f32, field: &str) -> Result<f32, CoreError> {
        let next = match self {
            Self::Set(v) => {
                if !v.is_finite() {
                    return Err(CoreError::Validation(format!("{field} must be finite")));
                }
                if v < min || v > max {
                    return Err(CoreError::Validation(format!(
                        "{field} must be within {min}..={max} (got {v})"
                    )));
                }
                v
            }
            Self::Add(delta) => {
                if !delta.is_finite() {
                    return Err(CoreError::Validation(format!(
                        "{field} delta must be finite"
                    )));
                }
                (current + delta).clamp(min, max)
            }
            Self::Scale(factor) => {
                if !factor.is_finite() {
                    return Err(CoreError::Validation(format!(
                        "{field} scale must be finite"
                    )));
                }
                (current * factor).clamp(min, max)
            }
        };
        Ok(next)
    }
}

impl<'de> Deserialize<'de> for ScalarOp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum Tagged {
            Set(f32),
            Add(f32),
            Scale(f32),
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Num(f32),
            Str(String),
            Obj(Tagged),
        }

        match Raw::deserialize(deserializer)? {
            Raw::Num(v) => Ok(Self::Set(v)),
            Raw::Str(s) => {
                let t = s.trim();
                if let Some(rest) = t.strip_prefix('+') {
                    let v: f32 = rest.parse().map_err(|_| {
                        serde::de::Error::custom(format!("invalid scalar delta '{s}'"))
                    })?;
                    Ok(Self::Add(v))
                } else if let Some(rest) = t.strip_prefix('-') {
                    let v: f32 = rest.parse().map_err(|_| {
                        serde::de::Error::custom(format!("invalid scalar delta '{s}'"))
                    })?;
                    Ok(Self::Add(-v))
                } else if let Some(rest) = t.strip_prefix('*') {
                    let v: f32 = rest.parse().map_err(|_| {
                        serde::de::Error::custom(format!("invalid scalar scale '{s}'"))
                    })?;
                    Ok(Self::Scale(v))
                } else {
                    let v: f32 = t
                        .parse()
                        .map_err(|_| serde::de::Error::custom(format!("invalid scalar '{s}'")))?;
                    Ok(Self::Set(v))
                }
            }
            Raw::Obj(Tagged::Set(v)) => Ok(Self::Set(v)),
            Raw::Obj(Tagged::Add(v)) => Ok(Self::Add(v)),
            Raw::Obj(Tagged::Scale(v)) => Ok(Self::Scale(v)),
        }
    }
}

/// Named HSL band keys (`red`..`magenta`) so agents never guess positional indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HslBandId {
    Red,
    Orange,
    Yellow,
    Green,
    Aqua,
    Blue,
    Purple,
    Magenta,
}

impl HslBandId {
    pub const fn index(self) -> usize {
        match self {
            Self::Red => 0,
            Self::Orange => 1,
            Self::Yellow => 2,
            Self::Green => 3,
            Self::Aqua => 4,
            Self::Blue => 5,
            Self::Purple => 6,
            Self::Magenta => 7,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HslBandPatch {
    pub hue: Option<ScalarOp>,
    pub saturation: Option<ScalarOp>,
    pub luminance: Option<ScalarOp>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ColorWheelPatch {
    pub hue_x: Option<ScalarOp>,
    pub hue_y: Option<ScalarOp>,
    pub luma: Option<ScalarOp>,
    pub reset: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WheelsPatch {
    #[serde(alias = "shadows_wheel")]
    pub shadows: Option<ColorWheelPatch>,
    #[serde(alias = "midtones_wheel")]
    pub midtones: Option<ColorWheelPatch>,
    #[serde(alias = "highlights_wheel")]
    pub highlights: Option<ColorWheelPatch>,
    #[serde(alias = "master_wheel")]
    pub master: Option<ColorWheelPatch>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CurveChannelPatch {
    Set(Vec<[f32; 2]>),
    Reset,
}

impl Serialize for CurveChannelPatch {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Set(pts) => pts.serialize(serializer),
            Self::Reset => serializer.serialize_str("reset"),
        }
    }
}

impl<'de> Deserialize<'de> for CurveChannelPatch {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let val = Value::deserialize(deserializer)?;
        if let Some(s) = val.as_str()
            && (s.eq_ignore_ascii_case("reset") || s.eq_ignore_ascii_case("identity"))
        {
            return Ok(Self::Reset);
        }
        if let Some(obj) = val.as_object()
            && let Some(inner) = obj.get("set")
            && obj.len() == 1
        {
            let pts = crate::registry::parse_curve_points_value(inner, "curves")
                .map_err(serde::de::Error::custom)?;
            return Ok(Self::Set(pts));
        }
        let pts = crate::registry::parse_curve_points_value(&val, "curves")
            .map_err(serde::de::Error::custom)?;
        Ok(Self::Set(pts))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToneCurvesPatch {
    pub rgb: Option<CurveChannelPatch>,
    pub luma: Option<CurveChannelPatch>,
    pub red: Option<CurveChannelPatch>,
    pub green: Option<CurveChannelPatch>,
    pub blue: Option<CurveChannelPatch>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChannelLevelsPatch {
    pub in_black: Option<ScalarOp>,
    pub in_mid: Option<ScalarOp>,
    pub in_white: Option<ScalarOp>,
    pub out_black: Option<ScalarOp>,
    pub out_white: Option<ScalarOp>,
    pub reset: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LevelsPatch {
    pub in_black: Option<ScalarOp>,
    pub in_mid: Option<ScalarOp>,
    pub in_white: Option<ScalarOp>,
    pub out_black: Option<ScalarOp>,
    pub out_white: Option<ScalarOp>,
    pub red: Option<ChannelLevelsPatch>,
    pub green: Option<ChannelLevelsPatch>,
    pub blue: Option<ChannelLevelsPatch>,
    pub reset: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BwPatch {
    pub enabled: Option<bool>,
    pub red: Option<ScalarOp>,
    pub yellow: Option<ScalarOp>,
    pub green: Option<ScalarOp>,
    pub cyan: Option<ScalarOp>,
    pub blue: Option<ScalarOp>,
    pub magenta: Option<ScalarOp>,
    pub split_shadow_hue: Option<ScalarOp>,
    pub split_shadow_sat: Option<ScalarOp>,
    pub split_highlight_hue: Option<ScalarOp>,
    pub split_highlight_sat: Option<ScalarOp>,
    pub reset: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RotateOp {
    Set(i8),
    Add(i8),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ColorEditorSlicePatch {
    pub enabled: Option<bool>,
    pub target_hue: Option<ScalarOp>,
    pub target_sat: Option<ScalarOp>,
    pub hue_range: Option<ScalarOp>,
    pub sat_range: Option<ScalarOp>,
    pub smoothness: Option<ScalarOp>,
    pub hue_shift: Option<ScalarOp>,
    pub saturation: Option<ScalarOp>,
    pub lightness: Option<ScalarOp>,
    pub uniformity: Option<ScalarOp>,
    pub reset: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SkinTonePatch {
    pub enabled: Option<bool>,
    pub target_hue: Option<ScalarOp>,
    pub target_sat: Option<ScalarOp>,
    pub target_lightness: Option<ScalarOp>,
    pub smoothness: Option<ScalarOp>,
    pub uniformity_hue: Option<ScalarOp>,
    pub uniformity_sat: Option<ScalarOp>,
    pub uniformity_lightness: Option<ScalarOp>,
    pub amount_hue: Option<ScalarOp>,
    pub amount_sat: Option<ScalarOp>,
    pub amount_lightness: Option<ScalarOp>,
    pub reset: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LensPatch {
    pub distortion: Option<ScalarOp>,
    pub chromatic_aberration: Option<ScalarOp>,
    pub purple_fringing: Option<ScalarOp>,
    pub light_falloff: Option<ScalarOp>,
    pub reset: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeometryPatch {
    pub crop: Option<[f32; 4]>,
    pub aspect_preset: Option<CropAspect>,
    pub straighten_deg: Option<ScalarOp>,
    pub rotate_90: Option<RotateOp>,
    pub flip_h: Option<bool>,
    pub flip_v: Option<bool>,
    pub grid: Option<CompositionGrid>,
    pub keystone_vertical: Option<ScalarOp>,
    pub keystone_horizontal: Option<ScalarOp>,
    pub keystone_aspect: Option<ScalarOp>,
    pub keystone_amount: Option<ScalarOp>,
    pub reset: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FilmNegativePatch {
    pub base_r: Option<ScalarOp>,
    pub base_g: Option<ScalarOp>,
    pub base_b: Option<ScalarOp>,
    pub density: Option<ScalarOp>,
    pub contrast: Option<ScalarOp>,
    pub black_clip: Option<ScalarOp>,
    pub white_clip: Option<ScalarOp>,
    pub reset: Option<bool>,
}

/// Typed sparse patch: agents send only the fields they want to modify.
/// Unknown fields fail loudly (`deny_unknown_fields`) so typos never silently no-op.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecipePatch {
    pub processing_mode: Option<ProcessingMode>,
    pub film_negative: Option<FilmNegativePatch>,
    pub film_curve: Option<FilmCurve>,
    pub exposure: Option<ScalarOp>,
    pub brightness: Option<ScalarOp>,
    pub contrast: Option<ScalarOp>,
    pub highlights: Option<ScalarOp>,
    pub shadows: Option<ScalarOp>,
    pub whites: Option<ScalarOp>,
    pub blacks: Option<ScalarOp>,
    pub wb_preset: Option<WbPreset>,
    pub temperature: Option<ScalarOp>,
    pub tint: Option<ScalarOp>,
    pub vibrance: Option<ScalarOp>,
    pub saturation: Option<ScalarOp>,
    pub clarity: Option<ScalarOp>,
    pub clarity_method: Option<ClarityMethod>,
    pub structure: Option<ScalarOp>,
    pub dehaze: Option<ScalarOp>,
    pub sharpen: Option<ScalarOp>,
    pub sharpen_radius: Option<ScalarOp>,
    pub sharpen_threshold: Option<ScalarOp>,
    pub sharpen_halo: Option<ScalarOp>,
    pub noise_reduction: Option<ScalarOp>,
    pub nr_color: Option<ScalarOp>,
    pub vignette: Option<ScalarOp>,
    pub vignette_method: Option<VignetteMethod>,
    pub grain: Option<ScalarOp>,
    pub grain_type: Option<GrainType>,
    pub grain_size: Option<ScalarOp>,
    pub shadows_wheel: Option<ColorWheelPatch>,
    pub midtones_wheel: Option<ColorWheelPatch>,
    pub highlights_wheel: Option<ColorWheelPatch>,
    pub master_wheel: Option<ColorWheelPatch>,
    pub wheels: Option<WheelsPatch>,
    pub curves: Option<ToneCurvesPatch>,
    pub hsl: Option<BTreeMap<HslBandId, HslBandPatch>>,
    pub advanced_color: Option<BTreeMap<String, ColorEditorSlicePatch>>,
    pub skin_tone: Option<SkinTonePatch>,
    pub levels: Option<LevelsPatch>,
    pub bw: Option<BwPatch>,
    pub lens: Option<LensPatch>,
    pub geometry: Option<GeometryPatch>,
    pub layers: Option<Vec<crate::AdjustmentLayer>>,
    pub heal_spots: Option<Vec<crate::HealSpot>>,
}

impl RecipePatch {
    /// Applies this sparse patch onto `recipe`, validating all ranges and returning field diffs.
    pub fn apply_to(
        &self,
        recipe: &mut DevelopRecipe,
        src_dims: Option<[u32; 2]>,
    ) -> Result<Vec<FieldDiff>, CoreError> {
        let before = recipe.clone();

        if let Some(pm) = self.processing_mode {
            recipe.processing_mode = pm;
        }
        if let Some(fn_p) = &self.film_negative {
            if fn_p.reset == Some(true) {
                recipe.film_negative = FilmNegativeParams::default();
            }
            if let Some(op) = fn_p.base_r {
                recipe.film_negative.base_r = op.apply(
                    recipe.film_negative.base_r,
                    0.05,
                    2.0,
                    "film_negative.base_r",
                )?;
            }
            if let Some(op) = fn_p.base_g {
                recipe.film_negative.base_g = op.apply(
                    recipe.film_negative.base_g,
                    0.05,
                    2.0,
                    "film_negative.base_g",
                )?;
            }
            if let Some(op) = fn_p.base_b {
                recipe.film_negative.base_b = op.apply(
                    recipe.film_negative.base_b,
                    0.05,
                    2.0,
                    "film_negative.base_b",
                )?;
            }
            if let Some(op) = fn_p.density {
                recipe.film_negative.density = op.apply(
                    recipe.film_negative.density,
                    -1.0,
                    1.0,
                    "film_negative.density",
                )?;
            }
            if let Some(op) = fn_p.contrast {
                recipe.film_negative.contrast = op.apply(
                    recipe.film_negative.contrast,
                    -1.0,
                    1.0,
                    "film_negative.contrast",
                )?;
            }
            if let Some(op) = fn_p.black_clip {
                recipe.film_negative.black_clip = op.apply(
                    recipe.film_negative.black_clip,
                    0.0,
                    0.45,
                    "film_negative.black_clip",
                )?;
            }
            if let Some(op) = fn_p.white_clip {
                recipe.film_negative.white_clip = op.apply(
                    recipe.film_negative.white_clip,
                    0.55,
                    1.0,
                    "film_negative.white_clip",
                )?;
            }
            recipe.film_negative = recipe.film_negative.sanitized();
        }
        if let Some(fc) = self.film_curve {
            recipe.film_curve = fc;
        }
        if let Some(wb) = self.wb_preset {
            recipe.apply_wb_preset(wb);
        }
        if let Some(cm) = self.clarity_method {
            recipe.clarity_method = cm;
        }
        if let Some(gt) = self.grain_type {
            recipe.grain_type = gt;
        }
        if let Some(vm) = self.vignette_method {
            recipe.vignette_method = vm;
        }

        macro_rules! apply_scalar {
            ($field:ident, $min:expr, $max:expr) => {
                if let Some(op) = self.$field {
                    recipe.$field = op.apply(recipe.$field, $min, $max, stringify!($field))?;
                }
            };
        }

        apply_scalar!(exposure, -5.0, 5.0);
        apply_scalar!(brightness, -1.0, 1.0);
        apply_scalar!(contrast, -1.0, 1.0);
        apply_scalar!(highlights, -1.0, 1.0);
        apply_scalar!(shadows, -1.0, 1.0);
        apply_scalar!(whites, -1.0, 1.0);
        apply_scalar!(blacks, -1.0, 1.0);
        apply_scalar!(temperature, -1.0, 1.0);
        apply_scalar!(tint, -1.0, 1.0);
        if self.temperature.is_some() || self.tint.is_some() {
            recipe.wb_preset = WbPreset::Custom;
        }
        apply_scalar!(vibrance, -1.0, 1.0);
        apply_scalar!(saturation, -1.0, 1.0);
        apply_scalar!(clarity, -1.0, 1.0);
        apply_scalar!(structure, -1.0, 1.0);
        apply_scalar!(dehaze, -1.0, 1.0);
        apply_scalar!(sharpen, 0.0, 1.0);
        apply_scalar!(sharpen_radius, 0.5, 4.0);
        apply_scalar!(sharpen_threshold, 0.0, 1.0);
        apply_scalar!(sharpen_halo, 0.0, 1.0);
        apply_scalar!(noise_reduction, 0.0, 1.0);
        apply_scalar!(nr_color, 0.0, 1.0);
        apply_scalar!(vignette, -1.0, 1.0);
        apply_scalar!(grain, 0.0, 1.0);
        apply_scalar!(grain_size, 0.0, 1.0);

        if let Some(wp) = &self.shadows_wheel {
            apply_wheel_patch(&mut recipe.shadows_wheel, wp, "shadows_wheel")?;
        }
        if let Some(wp) = &self.midtones_wheel {
            apply_wheel_patch(&mut recipe.midtones_wheel, wp, "midtones_wheel")?;
        }
        if let Some(wp) = &self.highlights_wheel {
            apply_wheel_patch(&mut recipe.highlights_wheel, wp, "highlights_wheel")?;
        }
        if let Some(wp) = &self.master_wheel {
            apply_wheel_patch(&mut recipe.master_wheel, wp, "master_wheel")?;
        }
        if let Some(wheels) = &self.wheels {
            if let Some(wp) = &wheels.shadows {
                apply_wheel_patch(&mut recipe.shadows_wheel, wp, "shadows_wheel")?;
            }
            if let Some(wp) = &wheels.midtones {
                apply_wheel_patch(&mut recipe.midtones_wheel, wp, "midtones_wheel")?;
            }
            if let Some(wp) = &wheels.highlights {
                apply_wheel_patch(&mut recipe.highlights_wheel, wp, "highlights_wheel")?;
            }
            if let Some(wp) = &wheels.master {
                apply_wheel_patch(&mut recipe.master_wheel, wp, "master_wheel")?;
            }
        }

        if let Some(cp) = &self.curves {
            if let Some(c) = &cp.rgb {
                recipe.curves.rgb = apply_curve_patch(c);
            }
            if let Some(c) = &cp.luma {
                recipe.curves.luma = apply_curve_patch(c);
            }
            if let Some(c) = &cp.red {
                recipe.curves.red = apply_curve_patch(c);
            }
            if let Some(c) = &cp.green {
                recipe.curves.green = apply_curve_patch(c);
            }
            if let Some(c) = &cp.blue {
                recipe.curves.blue = apply_curve_patch(c);
            }
        }

        if let Some(hsl_map) = &self.hsl {
            for (band_id, bp) in hsl_map {
                let slot = &mut recipe.hsl[band_id.index()];
                if let Some(op) = bp.hue {
                    slot.hue = op.apply(slot.hue, -1.0, 1.0, "hsl.hue")?;
                }
                if let Some(op) = bp.saturation {
                    slot.saturation = op.apply(slot.saturation, -1.0, 1.0, "hsl.saturation")?;
                }
                if let Some(op) = bp.luminance {
                    slot.luminance = op.apply(slot.luminance, -1.0, 1.0, "hsl.luminance")?;
                }
            }
        }

        if let Some(lp) = &self.levels {
            if lp.reset == Some(true) {
                recipe.levels = crate::LevelsParams::default();
            }
            if let Some(op) = lp.in_black {
                recipe.levels.in_black =
                    op.apply(recipe.levels.in_black, 0.0, 0.95, "levels.in_black")?;
            }
            if let Some(op) = lp.in_mid {
                recipe.levels.in_mid =
                    op.apply(recipe.levels.in_mid, -1.0, 1.0, "levels.in_mid")?;
            }
            if let Some(op) = lp.in_white {
                recipe.levels.in_white =
                    op.apply(recipe.levels.in_white, 0.05, 1.0, "levels.in_white")?;
            }
            if let Some(op) = lp.out_black {
                recipe.levels.out_black =
                    op.apply(recipe.levels.out_black, 0.0, 1.0, "levels.out_black")?;
            }
            if let Some(op) = lp.out_white {
                recipe.levels.out_white =
                    op.apply(recipe.levels.out_white, 0.0, 1.0, "levels.out_white")?;
            }
            if let Some(cp) = &lp.red {
                apply_channel_levels_patch(&mut recipe.levels.red, cp, "levels.red")?;
            }
            if let Some(cp) = &lp.green {
                apply_channel_levels_patch(&mut recipe.levels.green, cp, "levels.green")?;
            }
            if let Some(cp) = &lp.blue {
                apply_channel_levels_patch(&mut recipe.levels.blue, cp, "levels.blue")?;
            }
            recipe.levels = recipe.levels.sanitized();
        }

        if let Some(bp) = &self.bw {
            if bp.reset == Some(true) {
                recipe.bw = crate::BlackAndWhiteParams::default();
            }
            if let Some(en) = bp.enabled {
                recipe.bw.enabled = en;
            }
            if let Some(op) = bp.red {
                recipe.bw.red = op.apply(recipe.bw.red, -1.0, 1.0, "bw.red")?;
            }
            if let Some(op) = bp.yellow {
                recipe.bw.yellow = op.apply(recipe.bw.yellow, -1.0, 1.0, "bw.yellow")?;
            }
            if let Some(op) = bp.green {
                recipe.bw.green = op.apply(recipe.bw.green, -1.0, 1.0, "bw.green")?;
            }
            if let Some(op) = bp.cyan {
                recipe.bw.cyan = op.apply(recipe.bw.cyan, -1.0, 1.0, "bw.cyan")?;
            }
            if let Some(op) = bp.blue {
                recipe.bw.blue = op.apply(recipe.bw.blue, -1.0, 1.0, "bw.blue")?;
            }
            if let Some(op) = bp.magenta {
                recipe.bw.magenta = op.apply(recipe.bw.magenta, -1.0, 1.0, "bw.magenta")?;
            }
            if let Some(op) = bp.split_shadow_hue {
                recipe.bw.split_shadow_hue = op.apply(
                    recipe.bw.split_shadow_hue,
                    0.0,
                    360.0,
                    "bw.split_shadow_hue",
                )?;
            }
            if let Some(op) = bp.split_shadow_sat {
                recipe.bw.split_shadow_sat =
                    op.apply(recipe.bw.split_shadow_sat, 0.0, 1.0, "bw.split_shadow_sat")?;
            }
            if let Some(op) = bp.split_highlight_hue {
                recipe.bw.split_highlight_hue = op.apply(
                    recipe.bw.split_highlight_hue,
                    0.0,
                    360.0,
                    "bw.split_highlight_hue",
                )?;
            }
            if let Some(op) = bp.split_highlight_sat {
                recipe.bw.split_highlight_sat = op.apply(
                    recipe.bw.split_highlight_sat,
                    0.0,
                    1.0,
                    "bw.split_highlight_sat",
                )?;
            }
        }

        if let Some(adv_map) = &self.advanced_color {
            for (idx_key, sp) in adv_map {
                let idx: usize = idx_key.trim().parse().map_err(|_| {
                    CoreError::Validation(format!(
                        "Invalid advanced_color slice key '{idx_key}' (expected 0..{})",
                        ADVANCED_COLOR_SLICE_COUNT
                    ))
                })?;
                if idx >= ADVANCED_COLOR_SLICE_COUNT {
                    return Err(CoreError::Validation(format!(
                        "advanced_color slice index {idx} out of range (0..{})",
                        ADVANCED_COLOR_SLICE_COUNT
                    )));
                }
                let slot = &mut recipe.advanced_color[idx];
                if sp.reset == Some(true) {
                    *slot = crate::ColorEditorSlice::default();
                }
                if let Some(en) = sp.enabled {
                    slot.enabled = en;
                }
                if let Some(op) = sp.target_hue {
                    slot.target_hue =
                        op.apply(slot.target_hue, 0.0, 360.0, "advanced_color.target_hue")?;
                }
                if let Some(op) = sp.target_sat {
                    slot.target_sat =
                        op.apply(slot.target_sat, 0.0, 1.0, "advanced_color.target_sat")?;
                }
                if let Some(op) = sp.hue_range {
                    slot.hue_range =
                        op.apply(slot.hue_range, 5.0, 180.0, "advanced_color.hue_range")?;
                }
                if let Some(op) = sp.sat_range {
                    slot.sat_range =
                        op.apply(slot.sat_range, 0.05, 1.0, "advanced_color.sat_range")?;
                }
                if let Some(op) = sp.smoothness {
                    slot.smoothness =
                        op.apply(slot.smoothness, 0.0, 1.0, "advanced_color.smoothness")?;
                }
                if let Some(op) = sp.hue_shift {
                    slot.hue_shift =
                        op.apply(slot.hue_shift, -1.0, 1.0, "advanced_color.hue_shift")?;
                }
                if let Some(op) = sp.saturation {
                    slot.saturation =
                        op.apply(slot.saturation, -1.0, 1.0, "advanced_color.saturation")?;
                }
                if let Some(op) = sp.lightness {
                    slot.lightness =
                        op.apply(slot.lightness, -1.0, 1.0, "advanced_color.lightness")?;
                }
                if let Some(op) = sp.uniformity {
                    slot.uniformity =
                        op.apply(slot.uniformity, 0.0, 1.0, "advanced_color.uniformity")?;
                }
                *slot = slot.sanitized();
            }
        }

        if let Some(sk) = &self.skin_tone {
            if sk.reset == Some(true) {
                recipe.skin_tone = crate::SkinToneParams::default();
            }
            if let Some(en) = sk.enabled {
                recipe.skin_tone.enabled = en;
            }
            if let Some(op) = sk.target_hue {
                recipe.skin_tone.target_hue = op.apply(
                    recipe.skin_tone.target_hue,
                    0.0,
                    360.0,
                    "skin_tone.target_hue",
                )?;
            }
            if let Some(op) = sk.target_sat {
                recipe.skin_tone.target_sat = op.apply(
                    recipe.skin_tone.target_sat,
                    0.0,
                    1.0,
                    "skin_tone.target_sat",
                )?;
            }
            if let Some(op) = sk.target_lightness {
                recipe.skin_tone.target_lightness = op.apply(
                    recipe.skin_tone.target_lightness,
                    0.0,
                    1.0,
                    "skin_tone.target_lightness",
                )?;
            }
            if let Some(op) = sk.smoothness {
                recipe.skin_tone.smoothness = op.apply(
                    recipe.skin_tone.smoothness,
                    0.0,
                    1.0,
                    "skin_tone.smoothness",
                )?;
            }
            if let Some(op) = sk.uniformity_hue {
                recipe.skin_tone.uniformity_hue = op.apply(
                    recipe.skin_tone.uniformity_hue,
                    0.0,
                    1.0,
                    "skin_tone.uniformity_hue",
                )?;
            }
            if let Some(op) = sk.uniformity_sat {
                recipe.skin_tone.uniformity_sat = op.apply(
                    recipe.skin_tone.uniformity_sat,
                    0.0,
                    1.0,
                    "skin_tone.uniformity_sat",
                )?;
            }
            if let Some(op) = sk.uniformity_lightness {
                recipe.skin_tone.uniformity_lightness = op.apply(
                    recipe.skin_tone.uniformity_lightness,
                    0.0,
                    1.0,
                    "skin_tone.uniformity_lightness",
                )?;
            }
            if let Some(op) = sk.amount_hue {
                recipe.skin_tone.amount_hue = op.apply(
                    recipe.skin_tone.amount_hue,
                    -1.0,
                    1.0,
                    "skin_tone.amount_hue",
                )?;
            }
            if let Some(op) = sk.amount_sat {
                recipe.skin_tone.amount_sat = op.apply(
                    recipe.skin_tone.amount_sat,
                    -1.0,
                    1.0,
                    "skin_tone.amount_sat",
                )?;
            }
            if let Some(op) = sk.amount_lightness {
                recipe.skin_tone.amount_lightness = op.apply(
                    recipe.skin_tone.amount_lightness,
                    -1.0,
                    1.0,
                    "skin_tone.amount_lightness",
                )?;
            }
            recipe.skin_tone = recipe.skin_tone.sanitized();
        }

        if let Some(lp) = &self.lens {
            if lp.reset == Some(true) {
                recipe.lens = crate::LensParams::default();
            }
            if let Some(op) = lp.distortion {
                recipe.lens.distortion =
                    op.apply(recipe.lens.distortion, -1.0, 1.0, "lens.distortion")?;
            }
            if let Some(op) = lp.chromatic_aberration {
                recipe.lens.chromatic_aberration = op.apply(
                    recipe.lens.chromatic_aberration,
                    -1.0,
                    1.0,
                    "lens.chromatic_aberration",
                )?;
            }
            if let Some(op) = lp.purple_fringing {
                recipe.lens.purple_fringing = op.apply(
                    recipe.lens.purple_fringing,
                    0.0,
                    1.0,
                    "lens.purple_fringing",
                )?;
            }
            if let Some(op) = lp.light_falloff {
                recipe.lens.light_falloff =
                    op.apply(recipe.lens.light_falloff, 0.0, 1.0, "lens.light_falloff")?;
            }
            recipe.lens = recipe.lens.sanitized();
        }

        if let Some(gp) = &self.geometry {
            if gp.reset == Some(true) {
                recipe.geometry = crate::GeometryParams::default();
            }
            if let Some(rot) = gp.rotate_90 {
                match rot {
                    RotateOp::Set(t) => recipe.geometry.rotate_90 = t.rem_euclid(4),
                    RotateOp::Add(t) => {
                        recipe.geometry.rotate_90 = (recipe.geometry.rotate_90 + t).rem_euclid(4);
                    }
                }
            }
            if let Some(fh) = gp.flip_h {
                recipe.geometry.flip_h = fh;
            }
            if let Some(fv) = gp.flip_v {
                recipe.geometry.flip_v = fv;
            }
            if let Some(op) = gp.straighten_deg {
                recipe.geometry.straighten_deg = op.apply(
                    recipe.geometry.straighten_deg,
                    -45.0,
                    45.0,
                    "geometry.straighten_deg",
                )?;
            }
            if let Some(crop) = gp.crop {
                recipe.geometry.crop = crop;
                recipe.geometry.crop = recipe.geometry.sanitized_crop();
            }
            if let Some(aspect) = gp.aspect_preset {
                if let Some([w, h]) = src_dims {
                    recipe.geometry.apply_aspect_preset(aspect, w, h);
                } else {
                    recipe.geometry.aspect_preset = aspect;
                }
            }
            if let Some(grid) = gp.grid {
                recipe.geometry.grid = grid;
            }
            if let Some(op) = gp.keystone_vertical {
                recipe.geometry.keystone_vertical = op.apply(
                    recipe.geometry.keystone_vertical,
                    -1.0,
                    1.0,
                    "geometry.keystone_vertical",
                )?;
            }
            if let Some(op) = gp.keystone_horizontal {
                recipe.geometry.keystone_horizontal = op.apply(
                    recipe.geometry.keystone_horizontal,
                    -1.0,
                    1.0,
                    "geometry.keystone_horizontal",
                )?;
            }
            if let Some(op) = gp.keystone_aspect {
                recipe.geometry.keystone_aspect = op.apply(
                    recipe.geometry.keystone_aspect,
                    -0.5,
                    1.0,
                    "geometry.keystone_aspect",
                )?;
            }
            if let Some(op) = gp.keystone_amount {
                recipe.geometry.keystone_amount = op.apply(
                    recipe.geometry.keystone_amount,
                    0.0,
                    1.0,
                    "geometry.keystone_amount",
                )?;
            }
        }

        if let Some(layers) = &self.layers {
            recipe.layers = layers
                .iter()
                .take(crate::MAX_ADJUSTMENT_LAYERS)
                .map(crate::AdjustmentLayer::sanitized)
                .collect();
        }
        if let Some(spots) = &self.heal_spots {
            recipe.heal_spots = spots
                .iter()
                .take(crate::MAX_HEAL_SPOTS)
                .map(crate::HealSpot::sanitized)
                .collect();
        }

        let diffs = recipe_field_diffs(&before, recipe);
        if !diffs.is_empty() {
            recipe.preset_name = None;
        }
        Ok(diffs)
    }
}

fn apply_channel_levels_patch(
    ch: &mut crate::ChannelLevels,
    p: &ChannelLevelsPatch,
    name: &str,
) -> Result<(), CoreError> {
    if p.reset == Some(true) {
        *ch = crate::ChannelLevels::default();
    }
    if let Some(op) = p.in_black {
        ch.in_black = op.apply(ch.in_black, 0.0, 0.95, &format!("{name}.in_black"))?;
    }
    if let Some(op) = p.in_mid {
        ch.in_mid = op.apply(ch.in_mid, -1.0, 1.0, &format!("{name}.in_mid"))?;
    }
    if let Some(op) = p.in_white {
        ch.in_white = op.apply(ch.in_white, 0.05, 1.0, &format!("{name}.in_white"))?;
    }
    if let Some(op) = p.out_black {
        ch.out_black = op.apply(ch.out_black, 0.0, 1.0, &format!("{name}.out_black"))?;
    }
    if let Some(op) = p.out_white {
        ch.out_white = op.apply(ch.out_white, 0.0, 1.0, &format!("{name}.out_white"))?;
    }
    *ch = ch.sanitized();
    Ok(())
}

fn apply_wheel_patch(
    w: &mut crate::ColorWheel,
    p: &ColorWheelPatch,
    name: &str,
) -> Result<(), CoreError> {
    if p.reset == Some(true) {
        *w = crate::ColorWheel::default();
    }
    if let Some(op) = p.hue_x {
        w.hue_x = op.apply(w.hue_x, -1.0, 1.0, &format!("{name}.hue_x"))?;
    }
    if let Some(op) = p.hue_y {
        w.hue_y = op.apply(w.hue_y, -1.0, 1.0, &format!("{name}.hue_y"))?;
    }
    if let Some(op) = p.luma {
        w.luma = op.apply(w.luma, -1.0, 1.0, &format!("{name}.luma"))?;
    }
    *w = w.clamped();
    Ok(())
}

fn apply_curve_patch(patch: &CurveChannelPatch) -> Vec<[f32; 2]> {
    match patch {
        CurveChannelPatch::Reset => crate::default_curve_points(),
        CurveChannelPatch::Set(pts) => {
            if pts.len() < 2 {
                crate::default_curve_points()
            } else {
                let mut clean: Vec<[f32; 2]> = pts
                    .iter()
                    .map(|p| {
                        [
                            if p[0].is_finite() {
                                p[0].clamp(0.0, 1.0)
                            } else {
                                0.0
                            },
                            if p[1].is_finite() {
                                p[1].clamp(0.0, 1.0)
                            } else {
                                0.0
                            },
                        ]
                    })
                    .collect();
                clean.sort_by(|a, b| a[0].total_cmp(&b[0]));
                clean
            }
        }
    }
}

/// Converts a [`DevelopRecipe`] into a minimal sparse [`RecipePatch`] containing only
/// non-default adjustments (ideal for clean User Style serialization and stacking).
pub fn recipe_to_sparse_patch(recipe: &DevelopRecipe) -> RecipePatch {
    let r = recipe.clone().sanitized();
    let d = DevelopRecipe::default();
    let mut patch = RecipePatch::default();

    let scalar_opt = |cur: f32, def: f32| -> Option<ScalarOp> {
        if (cur - def).abs() > 1e-6 {
            Some(ScalarOp::Set(cur))
        } else {
            None
        }
    };

    if r.processing_mode != d.processing_mode {
        patch.processing_mode = Some(r.processing_mode);
    }
    if !r.film_negative.is_default() {
        let f = r.film_negative;
        let df = d.film_negative;
        patch.film_negative = Some(FilmNegativePatch {
            base_r: scalar_opt(f.base_r, df.base_r),
            base_g: scalar_opt(f.base_g, df.base_g),
            base_b: scalar_opt(f.base_b, df.base_b),
            density: scalar_opt(f.density, df.density),
            contrast: scalar_opt(f.contrast, df.contrast),
            black_clip: scalar_opt(f.black_clip, df.black_clip),
            white_clip: scalar_opt(f.white_clip, df.white_clip),
            reset: None,
        });
    }
    if r.film_curve != d.film_curve {
        patch.film_curve = Some(r.film_curve);
    }
    patch.exposure = scalar_opt(r.exposure, d.exposure);
    patch.brightness = scalar_opt(r.brightness, d.brightness);
    patch.contrast = scalar_opt(r.contrast, d.contrast);
    patch.highlights = scalar_opt(r.highlights, d.highlights);
    patch.shadows = scalar_opt(r.shadows, d.shadows);
    patch.whites = scalar_opt(r.whites, d.whites);
    patch.blacks = scalar_opt(r.blacks, d.blacks);
    if r.wb_preset != d.wb_preset {
        patch.wb_preset = Some(r.wb_preset);
    }
    patch.temperature = scalar_opt(r.temperature, d.temperature);
    patch.tint = scalar_opt(r.tint, d.tint);
    patch.vibrance = scalar_opt(r.vibrance, d.vibrance);
    patch.saturation = scalar_opt(r.saturation, d.saturation);
    patch.clarity = scalar_opt(r.clarity, d.clarity);
    if r.clarity_method != d.clarity_method {
        patch.clarity_method = Some(r.clarity_method);
    }
    patch.structure = scalar_opt(r.structure, d.structure);
    patch.dehaze = scalar_opt(r.dehaze, d.dehaze);
    patch.sharpen = scalar_opt(r.sharpen, d.sharpen);
    patch.sharpen_radius = scalar_opt(r.sharpen_radius, d.sharpen_radius);
    patch.sharpen_threshold = scalar_opt(r.sharpen_threshold, d.sharpen_threshold);
    patch.sharpen_halo = scalar_opt(r.sharpen_halo, d.sharpen_halo);
    patch.noise_reduction = scalar_opt(r.noise_reduction, d.noise_reduction);
    patch.nr_color = scalar_opt(r.nr_color, d.nr_color);
    patch.vignette = scalar_opt(r.vignette, d.vignette);
    if r.vignette_method != d.vignette_method {
        patch.vignette_method = Some(r.vignette_method);
    }
    patch.grain = scalar_opt(r.grain, d.grain);
    if r.grain_type != d.grain_type {
        patch.grain_type = Some(r.grain_type);
    }
    patch.grain_size = scalar_opt(r.grain_size, d.grain_size);

    let wheel_opt = |w: crate::ColorWheel| -> Option<ColorWheelPatch> {
        if w.is_neutral() {
            None
        } else {
            Some(ColorWheelPatch {
                hue_x: scalar_opt(w.hue_x, 0.0),
                hue_y: scalar_opt(w.hue_y, 0.0),
                luma: scalar_opt(w.luma, 0.0),
                reset: None,
            })
        }
    };
    patch.shadows_wheel = wheel_opt(r.shadows_wheel);
    patch.midtones_wheel = wheel_opt(r.midtones_wheel);
    patch.highlights_wheel = wheel_opt(r.highlights_wheel);
    patch.master_wheel = wheel_opt(r.master_wheel);

    if !r.curves.is_identity() {
        let mut cp = ToneCurvesPatch::default();
        if r.curves.rgb != d.curves.rgb {
            cp.rgb = Some(CurveChannelPatch::Set(r.curves.rgb.clone()));
        }
        if r.curves.luma != d.curves.luma {
            cp.luma = Some(CurveChannelPatch::Set(r.curves.luma.clone()));
        }
        if r.curves.red != d.curves.red {
            cp.red = Some(CurveChannelPatch::Set(r.curves.red.clone()));
        }
        if r.curves.green != d.curves.green {
            cp.green = Some(CurveChannelPatch::Set(r.curves.green.clone()));
        }
        if r.curves.blue != d.curves.blue {
            cp.blue = Some(CurveChannelPatch::Set(r.curves.blue.clone()));
        }
        patch.curves = Some(cp);
    }

    const BAND_IDS: [HslBandId; HSL_BAND_COUNT] = [
        HslBandId::Red,
        HslBandId::Orange,
        HslBandId::Yellow,
        HslBandId::Green,
        HslBandId::Aqua,
        HslBandId::Blue,
        HslBandId::Purple,
        HslBandId::Magenta,
    ];
    let mut hsl_map = BTreeMap::new();
    for (i, band) in r.hsl.iter().enumerate() {
        if !band.is_neutral() {
            hsl_map.insert(
                BAND_IDS[i],
                HslBandPatch {
                    hue: scalar_opt(band.hue, 0.0),
                    saturation: scalar_opt(band.saturation, 0.0),
                    luminance: scalar_opt(band.luminance, 0.0),
                },
            );
        }
    }
    if !hsl_map.is_empty() {
        patch.hsl = Some(hsl_map);
    }

    let mut adv_map = BTreeMap::new();
    let d_slice = crate::ColorEditorSlice::default();
    for (i, sl) in r.advanced_color.iter().enumerate() {
        if !sl.is_neutral() {
            adv_map.insert(
                i.to_string(),
                ColorEditorSlicePatch {
                    enabled: (sl.enabled != d_slice.enabled).then_some(sl.enabled),
                    target_hue: scalar_opt(sl.target_hue, d_slice.target_hue),
                    target_sat: scalar_opt(sl.target_sat, d_slice.target_sat),
                    hue_range: scalar_opt(sl.hue_range, d_slice.hue_range),
                    sat_range: scalar_opt(sl.sat_range, d_slice.sat_range),
                    smoothness: scalar_opt(sl.smoothness, d_slice.smoothness),
                    hue_shift: scalar_opt(sl.hue_shift, d_slice.hue_shift),
                    saturation: scalar_opt(sl.saturation, d_slice.saturation),
                    lightness: scalar_opt(sl.lightness, d_slice.lightness),
                    uniformity: scalar_opt(sl.uniformity, d_slice.uniformity),
                    reset: None,
                },
            );
        }
    }
    if !adv_map.is_empty() {
        patch.advanced_color = Some(adv_map);
    }

    if !r.skin_tone.is_neutral() {
        let st = r.skin_tone;
        let ds = d.skin_tone;
        patch.skin_tone = Some(SkinTonePatch {
            enabled: (st.enabled != ds.enabled).then_some(st.enabled),
            target_hue: scalar_opt(st.target_hue, ds.target_hue),
            target_sat: scalar_opt(st.target_sat, ds.target_sat),
            target_lightness: scalar_opt(st.target_lightness, ds.target_lightness),
            smoothness: scalar_opt(st.smoothness, ds.smoothness),
            uniformity_hue: scalar_opt(st.uniformity_hue, ds.uniformity_hue),
            uniformity_sat: scalar_opt(st.uniformity_sat, ds.uniformity_sat),
            uniformity_lightness: scalar_opt(st.uniformity_lightness, ds.uniformity_lightness),
            amount_hue: scalar_opt(st.amount_hue, ds.amount_hue),
            amount_sat: scalar_opt(st.amount_sat, ds.amount_sat),
            amount_lightness: scalar_opt(st.amount_lightness, ds.amount_lightness),
            reset: None,
        });
    }

    if !r.levels.is_identity() {
        let ch_opt = |ch: crate::ChannelLevels| -> Option<ChannelLevelsPatch> {
            let dc = crate::ChannelLevels::default();
            if ch.is_identity() {
                None
            } else {
                Some(ChannelLevelsPatch {
                    in_black: scalar_opt(ch.in_black, dc.in_black),
                    in_mid: scalar_opt(ch.in_mid, dc.in_mid),
                    in_white: scalar_opt(ch.in_white, dc.in_white),
                    out_black: scalar_opt(ch.out_black, dc.out_black),
                    out_white: scalar_opt(ch.out_white, dc.out_white),
                    reset: None,
                })
            }
        };
        patch.levels = Some(LevelsPatch {
            in_black: scalar_opt(r.levels.in_black, d.levels.in_black),
            in_mid: scalar_opt(r.levels.in_mid, d.levels.in_mid),
            in_white: scalar_opt(r.levels.in_white, d.levels.in_white),
            out_black: scalar_opt(r.levels.out_black, d.levels.out_black),
            out_white: scalar_opt(r.levels.out_white, d.levels.out_white),
            red: ch_opt(r.levels.red),
            green: ch_opt(r.levels.green),
            blue: ch_opt(r.levels.blue),
            reset: None,
        });
    }

    if !r.bw.is_neutral() {
        patch.bw = Some(BwPatch {
            enabled: (r.bw.enabled != d.bw.enabled).then_some(r.bw.enabled),
            red: scalar_opt(r.bw.red, d.bw.red),
            yellow: scalar_opt(r.bw.yellow, d.bw.yellow),
            green: scalar_opt(r.bw.green, d.bw.green),
            cyan: scalar_opt(r.bw.cyan, d.bw.cyan),
            blue: scalar_opt(r.bw.blue, d.bw.blue),
            magenta: scalar_opt(r.bw.magenta, d.bw.magenta),
            split_shadow_hue: scalar_opt(r.bw.split_shadow_hue, d.bw.split_shadow_hue),
            split_shadow_sat: scalar_opt(r.bw.split_shadow_sat, d.bw.split_shadow_sat),
            split_highlight_hue: scalar_opt(r.bw.split_highlight_hue, d.bw.split_highlight_hue),
            split_highlight_sat: scalar_opt(r.bw.split_highlight_sat, d.bw.split_highlight_sat),
            reset: None,
        });
    }

    if !r.lens.is_identity() {
        patch.lens = Some(LensPatch {
            distortion: scalar_opt(r.lens.distortion, d.lens.distortion),
            chromatic_aberration: scalar_opt(
                r.lens.chromatic_aberration,
                d.lens.chromatic_aberration,
            ),
            purple_fringing: scalar_opt(r.lens.purple_fringing, d.lens.purple_fringing),
            light_falloff: scalar_opt(r.lens.light_falloff, d.lens.light_falloff),
            reset: None,
        });
    }

    if !r.geometry.is_identity() || r.geometry.aspect_preset != d.geometry.aspect_preset {
        let g = r.geometry;
        let dg = d.geometry;
        patch.geometry = Some(GeometryPatch {
            crop: (g.crop != dg.crop).then_some(g.crop),
            aspect_preset: (g.aspect_preset != dg.aspect_preset).then_some(g.aspect_preset),
            straighten_deg: scalar_opt(g.straighten_deg, dg.straighten_deg),
            rotate_90: (g.rotate_90 != dg.rotate_90).then_some(RotateOp::Set(g.rotate_90)),
            flip_h: (g.flip_h != dg.flip_h).then_some(g.flip_h),
            flip_v: (g.flip_v != dg.flip_v).then_some(g.flip_v),
            grid: (g.grid != dg.grid).then_some(g.grid),
            keystone_vertical: scalar_opt(g.keystone_vertical, dg.keystone_vertical),
            keystone_horizontal: scalar_opt(g.keystone_horizontal, dg.keystone_horizontal),
            keystone_aspect: scalar_opt(g.keystone_aspect, dg.keystone_aspect),
            keystone_amount: scalar_opt(g.keystone_amount, dg.keystone_amount),
            reset: None,
        });
    }

    if !r.layers.is_empty() {
        patch.layers = Some(r.layers);
    }
    if !r.heal_spots.is_empty() {
        patch.heal_spots = Some(r.heal_spots);
    }

    patch
}

/// Blends `base` toward `target` by `mix` in `0.0..=1.0`.
pub fn blend_recipes(base: &DevelopRecipe, target: &DevelopRecipe, mix: f32) -> DevelopRecipe {
    let t = if mix.is_finite() {
        mix.clamp(0.0, 1.0)
    } else {
        1.0
    };
    if (t - 1.0).abs() <= 1e-5 {
        return target.clone();
    }
    if t <= 1e-5 {
        return base.clone();
    }
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    let mut out = base.clone();
    for spec in SCALAR_PARAMS {
        let v = lerp((spec.get)(base), (spec.get)(target));
        (spec.set)(&mut out, v.clamp(spec.min, spec.max));
    }
    let lerp_wheel = |a: crate::ColorWheel, b: crate::ColorWheel| {
        crate::ColorWheel::new(
            lerp(a.hue_x, b.hue_x),
            lerp(a.hue_y, b.hue_y),
            lerp(a.luma, b.luma),
        )
        .clamped()
    };
    out.shadows_wheel = lerp_wheel(base.shadows_wheel, target.shadows_wheel);
    out.midtones_wheel = lerp_wheel(base.midtones_wheel, target.midtones_wheel);
    out.highlights_wheel = lerp_wheel(base.highlights_wheel, target.highlights_wheel);
    out.master_wheel = lerp_wheel(base.master_wheel, target.master_wheel);
    for i in 0..HSL_BAND_COUNT {
        out.hsl[i] = crate::HslBand::new(
            lerp(base.hsl[i].hue, target.hsl[i].hue),
            lerp(base.hsl[i].saturation, target.hsl[i].saturation),
            lerp(base.hsl[i].luminance, target.hsl[i].luminance),
        );
    }
    let lerp_ch = |a: crate::ChannelLevels, b: crate::ChannelLevels| crate::ChannelLevels {
        in_black: lerp(a.in_black, b.in_black),
        in_mid: lerp(a.in_mid, b.in_mid),
        in_white: lerp(a.in_white, b.in_white),
        out_black: lerp(a.out_black, b.out_black),
        out_white: lerp(a.out_white, b.out_white),
    };
    out.levels = crate::LevelsParams {
        in_black: lerp(base.levels.in_black, target.levels.in_black),
        in_mid: lerp(base.levels.in_mid, target.levels.in_mid),
        in_white: lerp(base.levels.in_white, target.levels.in_white),
        out_black: lerp(base.levels.out_black, target.levels.out_black),
        out_white: lerp(base.levels.out_white, target.levels.out_white),
        red: lerp_ch(base.levels.red, target.levels.red),
        green: lerp_ch(base.levels.green, target.levels.green),
        blue: lerp_ch(base.levels.blue, target.levels.blue),
    }
    .sanitized();
    if t >= 0.5 {
        out.processing_mode = target.processing_mode;
        out.film_curve = target.film_curve;
        out.wb_preset = target.wb_preset;
        out.clarity_method = target.clarity_method;
        out.grain_type = target.grain_type;
        out.vignette_method = target.vignette_method;
        out.curves = target.curves.clone();
        out.bw.enabled = target.bw.enabled;
        out.skin_tone.enabled = target.skin_tone.enabled;
    }
    for i in 0..ADVANCED_COLOR_SLICE_COUNT {
        let a = base.advanced_color[i];
        let b = target.advanced_color[i];
        out.advanced_color[i] = crate::ColorEditorSlice {
            enabled: if t >= 0.5 { b.enabled } else { a.enabled },
            target_hue: if t >= 0.5 { b.target_hue } else { a.target_hue },
            target_sat: lerp(a.target_sat, b.target_sat),
            hue_range: lerp(a.hue_range, b.hue_range),
            sat_range: lerp(a.sat_range, b.sat_range),
            smoothness: lerp(a.smoothness, b.smoothness),
            hue_shift: lerp(a.hue_shift, b.hue_shift),
            saturation: lerp(a.saturation, b.saturation),
            lightness: lerp(a.lightness, b.lightness),
            uniformity: lerp(a.uniformity, b.uniformity),
        }
        .sanitized();
    }
    out.skin_tone = crate::SkinToneParams {
        enabled: out.skin_tone.enabled,
        target_hue: if t >= 0.5 {
            target.skin_tone.target_hue
        } else {
            base.skin_tone.target_hue
        },
        target_sat: lerp(base.skin_tone.target_sat, target.skin_tone.target_sat),
        target_lightness: lerp(
            base.skin_tone.target_lightness,
            target.skin_tone.target_lightness,
        ),
        smoothness: lerp(base.skin_tone.smoothness, target.skin_tone.smoothness),
        uniformity_hue: lerp(
            base.skin_tone.uniformity_hue,
            target.skin_tone.uniformity_hue,
        ),
        uniformity_sat: lerp(
            base.skin_tone.uniformity_sat,
            target.skin_tone.uniformity_sat,
        ),
        uniformity_lightness: lerp(
            base.skin_tone.uniformity_lightness,
            target.skin_tone.uniformity_lightness,
        ),
        amount_hue: lerp(base.skin_tone.amount_hue, target.skin_tone.amount_hue),
        amount_sat: lerp(base.skin_tone.amount_sat, target.skin_tone.amount_sat),
        amount_lightness: lerp(
            base.skin_tone.amount_lightness,
            target.skin_tone.amount_lightness,
        ),
    }
    .sanitized();
    out.lens = crate::LensParams {
        distortion: lerp(base.lens.distortion, target.lens.distortion),
        chromatic_aberration: lerp(
            base.lens.chromatic_aberration,
            target.lens.chromatic_aberration,
        ),
        purple_fringing: lerp(base.lens.purple_fringing, target.lens.purple_fringing),
        light_falloff: lerp(base.lens.light_falloff, target.lens.light_falloff),
    }
    .sanitized();
    out.film_negative = FilmNegativeParams {
        base_r: lerp(base.film_negative.base_r, target.film_negative.base_r),
        base_g: lerp(base.film_negative.base_g, target.film_negative.base_g),
        base_b: lerp(base.film_negative.base_b, target.film_negative.base_b),
        density: lerp(base.film_negative.density, target.film_negative.density),
        contrast: lerp(base.film_negative.contrast, target.film_negative.contrast),
        black_clip: lerp(
            base.film_negative.black_clip,
            target.film_negative.black_clip,
        ),
        white_clip: lerp(
            base.film_negative.white_clip,
            target.film_negative.white_clip,
        ),
    }
    .sanitized();
    out.bw.red = lerp(base.bw.red, target.bw.red);
    out.bw.yellow = lerp(base.bw.yellow, target.bw.yellow);
    out.bw.green = lerp(base.bw.green, target.bw.green);
    out.bw.cyan = lerp(base.bw.cyan, target.bw.cyan);
    out.bw.blue = lerp(base.bw.blue, target.bw.blue);
    out.bw.magenta = lerp(base.bw.magenta, target.bw.magenta);
    out.bw.split_shadow_hue = lerp(base.bw.split_shadow_hue, target.bw.split_shadow_hue);
    out.bw.split_shadow_sat = lerp(base.bw.split_shadow_sat, target.bw.split_shadow_sat);
    out.bw.split_highlight_hue = lerp(base.bw.split_highlight_hue, target.bw.split_highlight_hue);
    out.bw.split_highlight_sat = lerp(base.bw.split_highlight_sat, target.bw.split_highlight_sat);
    out.preset_name = None;
    out
}

/// Resolves a built-in preset ID (case-insensitive) or returns `UnknownPreset`.
pub fn resolve_builtin_preset(id: &str) -> Result<DevelopRecipe, CoreError> {
    let needle = id.trim().to_ascii_lowercase().replace('-', "_");
    builtin_presets()
        .into_iter()
        .find(|p| p.id == needle || p.name.eq_ignore_ascii_case(id.trim()))
        .map(|p| p.recipe)
        .ok_or_else(|| CoreError::UnknownPreset(id.to_owned()))
}

/// Cross-frame reference matching parameters (Phase C batch solver).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MatchReferenceOp {
    pub copy_look: bool,
    pub match_exposure: bool,
    pub match_wb: bool,
    pub strength: f32,
    pub max_delta_stops: Option<f32>,
    pub highlight_guard: bool,
}

impl Default for MatchReferenceOp {
    fn default() -> Self {
        Self {
            copy_look: true,
            match_exposure: true,
            match_wb: true,
            strength: 1.0,
            max_delta_stops: Some(1.5),
            highlight_guard: true,
        }
    }
}

/// Composable edit operation evaluated sequentially in CLI `edit` and `batch run`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EditOp {
    Reset {
        #[serde(default)]
        keep_geometry: bool,
    },
    ApplyPreset {
        id: String,
        #[serde(default)]
        preserve_wb: bool,
        #[serde(default = "default_true")]
        preserve_geometry: bool,
        #[serde(default = "default_one")]
        mix: f32,
    },
    Patch(Box<RecipePatch>),
    AutoWb,
    PickWb {
        x_norm: f32,
        y_norm: f32,
    },
    AutoTone {
        #[serde(default = "default_true")]
        exposure: bool,
        #[serde(default = "default_true")]
        contrast: bool,
        #[serde(default = "default_true")]
        highlights_shadows: bool,
    },
    AutoLevels,
    MatchReference(MatchReferenceOp),
    SetMeta {
        rating: Option<u8>,
        flag: Option<CullFlag>,
        #[serde(default)]
        color_tag: Option<ColorTag>,
    },
}

/// Provenance actor recorded in `.rune-edit.json` sidecar history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EditActor {
    Human,
    Agent {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        job_id: Option<String>,
    },
    Auto {
        rule: String,
    },
}

/// Single field mutation recorded in edit diffs and provenance history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldDiff {
    pub field: String,
    pub before: Value,
    pub after: Value,
}

/// Persistent audit/revert entry stored inside `PhotoMeta.history` (capped at 64 entries).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditHistoryEntry {
    pub id: u64,
    pub timestamp_unix_s: u64,
    pub actor: EditActor,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diffs: Vec<FieldDiff>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe_before: Option<DevelopRecipe>,
}

/// Declarative batch manifest schema (`rd batch run <job.json>`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchJob {
    pub schema_version: u32,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    pub inputs: BatchInputs,
    #[serde(default)]
    pub pipeline: Vec<ConditionalRule>,
    #[serde(default)]
    pub sidecar_mode: SidecarWriteMode,
    #[serde(default)]
    pub exports: Vec<ExportTargetSpec>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BatchInputs {
    pub paths: Vec<PathBuf>,
    pub recursive: bool,
    pub picks_only: bool,
    pub min_rating: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConditionalRule {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub when: Option<ConditionExpr>,
    pub ops: Vec<EditOp>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionExpr {
    All(Vec<ConditionExpr>),
    Any(Vec<ConditionExpr>),
    Not(Box<ConditionExpr>),
    IsoGte(u32),
    IsoLte(u32),
    FlagIs(CullFlag),
    RatingGte(u8),
    HighlightClipPctGt(f32),
    ShadowClipPctGt(f32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidecarWriteMode {
    #[default]
    Update,
    CreateIfMissing,
    MemoryOnly,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportTargetSpec {
    pub id: String,
    pub output_dir: PathBuf,
    #[serde(default = "default_filename_template")]
    pub filename_template: String,
    pub format: String,
    #[serde(default = "default_jpeg_quality")]
    pub quality: u8,
    #[serde(default)]
    pub max_edge: Option<u32>,
    #[serde(default = "default_true")]
    pub respect_crop: bool,
    #[serde(default)]
    pub output_sharpen: f32,
}

fn default_filename_template() -> String {
    "{stem}.{ext}".to_owned()
}

const fn default_jpeg_quality() -> u8 {
    92
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_patch_supports_bare_numbers_and_relative_nudges() {
        let json = r#"{
            "exposure": "+0.35",
            "highlights": -0.40,
            "hsl": { "orange": { "saturation": "-0.15" } },
            "geometry": { "aspect_preset": "FourFive" }
        }"#;
        let patch: RecipePatch = serde_json::from_str(json).expect("valid patch");
        let mut recipe = DevelopRecipe {
            exposure: 0.25,
            ..DevelopRecipe::default()
        };
        let diffs = patch.apply_to(&mut recipe, Some([6000, 4000])).unwrap();
        assert!((recipe.exposure - 0.60).abs() < 1e-5);
        assert!((recipe.highlights + 0.40).abs() < 1e-5);
        assert!((recipe.hsl[1].saturation + 0.15).abs() < 1e-5);
        assert_eq!(recipe.geometry.aspect_preset, CropAspect::FourFive);
        assert!(diffs.len() >= 4);
    }

    #[test]
    fn unknown_patch_field_fails_loudly() {
        let bad = r#"{"warmth": 0.2}"#;
        let res: Result<RecipePatch, _> = serde_json::from_str(bad);
        assert!(res.is_err());
    }

    #[test]
    fn phase1_recipe_patch_and_blend_support() {
        let json = r#"{
            "film_curve": "film_high_contrast",
            "wb_preset": "cloudy",
            "clarity_method": "neutral",
            "sharpen_radius": 2.4,
            "sharpen_threshold": 0.15,
            "sharpen_halo": 0.4,
            "nr_color": 0.55,
            "grain_type": "cubic",
            "grain_size": 0.70,
            "wheels": { "master": { "hue_x": 0.2, "luma": 0.1 } },
            "curves": { "rgb": [[0.0, 0.02], [0.5, 0.55], [1.0, 0.98]] },
            "levels": { "red": { "in_black": 0.05, "in_mid": 0.2 } },
            "bw": { "enabled": true, "split_shadow_hue": 215.0, "split_shadow_sat": 0.45 }
        }"#;
        let patch: RecipePatch = serde_json::from_str(json).expect("valid phase1 patch");
        let mut recipe = DevelopRecipe::default();
        let diffs = patch
            .apply_to(&mut recipe, None)
            .expect("apply phase1 patch");
        assert!(!diffs.is_empty());
        assert_eq!(recipe.film_curve, FilmCurve::FilmHighContrast);
        assert_eq!(recipe.wb_preset, WbPreset::Cloudy);
        assert_eq!(recipe.clarity_method, ClarityMethod::Neutral);
        assert!((recipe.sharpen_radius - 2.4).abs() < 1e-5);
        assert!((recipe.nr_color - 0.55).abs() < 1e-5);
        assert_eq!(recipe.grain_type, GrainType::Cubic);
        assert!((recipe.grain_size - 0.70).abs() < 1e-5);
        assert!((recipe.master_wheel.hue_x - 0.2).abs() < 1e-5);
        assert_eq!(recipe.curves.rgb.len(), 3);
        assert!((recipe.levels.red.in_black - 0.05).abs() < 1e-5);
        assert!((recipe.bw.split_shadow_hue - 215.0).abs() < 1e-5);

        let blended = blend_recipes(&DevelopRecipe::default(), &recipe, 0.5);
        assert_eq!(blended.film_curve, FilmCurve::FilmHighContrast);
        assert!((blended.master_wheel.hue_x - 0.1).abs() < 1e-5);
        assert!((blended.levels.red.in_black - 0.025).abs() < 1e-5);
        assert!((blended.bw.split_shadow_sat - 0.225).abs() < 1e-5);

        // Patching temperature/tint transitions wb_preset to Custom (matching set_param_by_str & GUI).
        let temp_patch: RecipePatch =
            serde_json::from_str(r#"{"temperature": 0.45}"#).expect("temp patch");
        temp_patch
            .apply_to(&mut recipe, None)
            .expect("apply temp patch");
        assert_eq!(recipe.wb_preset, WbPreset::Custom);
        assert!((recipe.temperature - 0.45).abs() < 1e-5);

        // Levels in_black > 0.95 and in_white < 0.05 fail validation loudly (matching sanitized bounds).
        let bad_black: RecipePatch =
            serde_json::from_str(r#"{"levels": {"in_black": 0.97}}"#).unwrap();
        assert!(bad_black.apply_to(&mut recipe, None).is_err());
        let bad_red_white: RecipePatch =
            serde_json::from_str(r#"{"levels": {"red": {"in_white": 0.03}}}"#).unwrap();
        assert!(bad_red_white.apply_to(&mut recipe, None).is_err());
    }

    #[test]
    fn phase2_recipe_patch_and_blend_support() {
        let json = r#"{
            "vignette_method": "circular",
            "advanced_color": {
                "0": { "enabled": true, "target_hue": 205.0, "hue_range": 40.0, "uniformity": 0.6 }
            },
            "skin_tone": {
                "enabled": true,
                "uniformity_hue": 0.75,
                "amount_sat": -0.1
            },
            "lens": {
                "distortion": 0.25,
                "chromatic_aberration": -0.3,
                "purple_fringing": 0.5,
                "light_falloff": 0.4
            },
            "geometry": {
                "keystone_vertical": 0.35,
                "keystone_horizontal": -0.15,
                "keystone_aspect": 0.2,
                "keystone_amount": 0.9
            }
        }"#;
        let patch: RecipePatch = serde_json::from_str(json).expect("valid phase2 patch");
        let mut recipe = DevelopRecipe::default();
        let diffs = patch
            .apply_to(&mut recipe, None)
            .expect("apply phase2 patch");
        assert!(!diffs.is_empty());
        assert_eq!(recipe.vignette_method, VignetteMethod::Circular);
        assert!(recipe.advanced_color[0].enabled);
        assert!((recipe.advanced_color[0].uniformity - 0.6).abs() < 1e-5);
        assert!(recipe.skin_tone.enabled);
        assert!((recipe.skin_tone.uniformity_hue - 0.75).abs() < 1e-5);
        assert!((recipe.lens.distortion - 0.25).abs() < 1e-5);
        assert!((recipe.geometry.keystone_vertical - 0.35).abs() < 1e-5);

        let blended = blend_recipes(&DevelopRecipe::default(), &recipe, 0.5);
        assert_eq!(blended.vignette_method, VignetteMethod::Circular);
        assert!((blended.lens.distortion - 0.125).abs() < 1e-5);
        assert!((blended.skin_tone.uniformity_hue - 0.375).abs() < 1e-5);

        // Verify serde_json::from_value (MCP) and internally-tagged EditOp::Patch (CLI batch)
        // both deserialize string map keys for `advanced_color`.
        let val: Value = serde_json::from_str(json).expect("json value");
        let patch_from_val: RecipePatch =
            serde_json::from_value(val).expect("from_value RecipePatch");
        assert!(patch_from_val.advanced_color.is_some());

        let batch_op_json =
            r#"{"op": "patch", "advanced_color": {"1": {"enabled": true, "uniformity": 0.5}}}"#;
        let op: EditOp = serde_json::from_str(batch_op_json).expect("EditOp::Patch");
        if let EditOp::Patch(p) = op {
            let mut r2 = DevelopRecipe::default();
            p.apply_to(&mut r2, None).expect("apply EditOp::Patch");
            assert!(r2.advanced_color[1].enabled);
            assert!((r2.advanced_color[1].uniformity - 0.5).abs() < 1e-5);
        } else {
            panic!("expected EditOp::Patch");
        }
    }
}
