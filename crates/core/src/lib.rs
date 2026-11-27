//! UI- and GPU-agnostic core for Rune Editor.
//!
//! Owns the non-destructive photographic develop recipe, sidecar serialization,
//! Capture One 3-Way Color Balance, Fritsch-Carlson RGB/Luma Tone Curves,
//! 8-Band HSL Color Editor, session catalog & culling model, undo/redo history,
//! built-in styles, and scene-referred linear color math (including fast
//! precompiled evaluation for live histograms and full-resolution export).

pub mod analysis;
pub mod color_science;
pub mod curves;
pub mod geometry;
pub mod layers;
pub mod patch;
pub mod profile_io;
pub mod profiles;
pub mod registry;
pub mod session;
pub mod styles;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use crate::analysis::{
    ImageAnalysis, RecipeDiff, analyze_linear_proxy, ciede2000, diff_recipes,
};
pub use crate::color_science::{
    ADVANCED_COLOR_SLICE_COUNT, BlackAndWhiteParams, ChannelLevels, ColorEditorSlice,
    FilmNegativeParams, LevelsParams, ProcessingMode, SkinToneParams, WbPreset,
    auto_exposure_and_tone, auto_levels, auto_white_balance, invert_film_negative_linear,
    kelvin_to_temperature, pick_white_balance, rgb_to_hsl_and_lab, sample_color_slice_from_rgb,
    sample_film_base_from_rgb, temperature_to_kelvin, wb_preset_values,
};
pub use crate::curves::{CURVE_LUT_SIZE, ToneCurves, default_curve_points, sample_curve_lut};
pub use crate::geometry::{
    CompositionGrid, CropAspect, GeometryParams, LensParams, apply_lens_distortion_uv,
};
pub use crate::layers::{
    AdjustmentLayer, BrushDab, HealMode, HealSpot, LAYER_MASK_ATLAS_HEIGHT, LAYER_MASK_ATLAS_WIDTH,
    LAYER_MASK_TILE_SIZE, LayerLumaRangeSpec, LayerMaskAtlas, LayerMaskItem, LocalAdjustments,
    MAX_ADJUSTMENT_LAYERS, MAX_BRUSH_DABS_PER_MASK, MAX_HEAL_SPOTS, MAX_MASKS_PER_LAYER,
    MaskCombineOp, MaskShape, apply_local_adjustments_linear, bake_layer_mask_atlas, decode_base64,
    encode_base64, evaluate_spatial_mask_shape, luma_range_mask,
};
pub use crate::patch::{
    BatchJob, EditActor, EditHistoryEntry, EditOp, FieldDiff, HslBandId, MatchReferenceOp,
    RecipePatch, ScalarOp, blend_recipes, recipe_to_sparse_patch, resolve_builtin_preset,
};
pub use crate::profile_io::{
    ALLOWED_PROFILE_EXTENSIONS, CubeLut3D, DcpHsvTable, DcpProfile, IccClut3D, IccPcs, IccProfile,
    IccTrc, MAX_CUBE_LUT_SIZE, MAX_DCP_HUE_DIVS, MAX_DCP_IFD_ENTRIES, MAX_DCP_SAT_DIVS,
    MAX_DCP_TONE_CURVE_POINTS, MAX_DCP_VAL_DIVS, MAX_ICC_CLUT_GRID, MAX_ICC_CURVE_ENTRIES,
    MAX_ICC_TAG_COUNT, MAX_PROFILE_FILE_BYTES, USER_PROFILE_PREFIX, default_profiles_dir,
    delete_user_profile, delete_user_profile_in_dir, import_user_profile,
    import_user_profile_in_dir, import_user_profile_in_dir_with_overwrite,
    list_all_camera_profiles, list_user_profiles, list_user_profiles_in_dir, parse_cube_lut,
    parse_dcp_profile, parse_icc_profile, resolve_profile_lut, sanitize_profile_slug,
};
pub use crate::profiles::{
    BUILTIN_CAMERA_PROFILES, BuiltinCameraProfileSpec, CameraCalibration, CameraProfileDescriptor,
    DEFAULT_CAMERA_PROFILE, DEFAULT_PROFILE_STRENGTH, ILLUMINANT_D65_KELVIN,
    ILLUMINANT_STD_A_KELVIN, LUT_SHAPER_KNEE, LUT_SHAPER_LOG_DENOM, LUT_SHAPER_MAX_LINEAR,
    PROFILE_LUT_SIZE, ProfileCategory, ProfileLut3D, bake_builtin_profile_lut,
    bradford_adaptation_matrix, builtin_camera_profiles, cached_builtin_profile_lut,
    camera_to_linear_srgb_matrix, canonical_profile_id, default_camera_profile,
    default_profile_strength, find_builtin_profile, interpolate_camera_matrix,
    is_valid_camera_profile_id, kelvin_to_xyz_whitepoint, linear_srgb_to_oklab, lut_shaper_forward,
    lut_shaper_inverse, oklab_to_linear_srgb_gamut_safe,
};
pub use crate::registry::{
    ALL_CLIPBOARD_GROUPS, ParamDescriptor, ParamGroup, SCALAR_PARAMS, ScalarParamSpec,
    all_param_descriptors, canonical_param_key, copy_recipe_groups, find_scalar_param,
    get_scalar_by_path, modified_recipe_groups, parse_hsl_band_index, recipe_field_diffs,
    recipe_non_default_map, recipe_schema_json, reset_recipe_group, search_params,
    set_param_by_str,
};
pub use crate::session::{
    ColorTag, CullFilter, CullFlag, META_SCHEMA_VERSION, PhotoMeta, PhotoVariant, SessionCatalog,
    SessionEntry, SessionFolderLayout, create_capture_one_session, move_photo_with_sidecar,
};
pub use crate::styles::{
    UserStyle, apply_style_to_recipe, default_styles_dir, delete_user_style,
    delete_user_style_in_dir, list_user_styles, list_user_styles_in_dir, load_user_style,
    load_user_style_in_dir, sanitize_style_slug, save_user_style, save_user_style_in_dir,
    stack_style_onto_recipe,
};

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

    /// Clamps `(hue_x, hue_y)` to the unit disk and `luma` to `-1.0..=1.0`,
    /// sanitizing any non-finite inputs to `0.0`.
    pub fn clamped(self) -> Self {
        let raw_x = if self.hue_x.is_finite() {
            self.hue_x
        } else {
            0.0
        };
        let raw_y = if self.hue_y.is_finite() {
            self.hue_y
        } else {
            0.0
        };
        let raw_l = if self.luma.is_finite() {
            self.luma
        } else {
            0.0
        };
        let len = raw_x.hypot(raw_y);
        let (hx, hy) = if len > 1.0 {
            (raw_x / len, raw_y / len)
        } else {
            (raw_x, raw_y)
        };
        Self {
            hue_x: hx,
            hue_y: hy,
            luma: raw_l.clamp(-1.0, 1.0),
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

const fn default_one() -> f32 {
    1.0
}

const fn default_grain_size() -> f32 {
    0.35
}

/// Capture One-style Base Characteristics Film Curve response.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilmCurve {
    #[default]
    FilmStandard,
    FilmExtraShadow,
    FilmHighContrast,
    LinearResponse,
}

impl FilmCurve {
    pub const ALL: [Self; 4] = [
        Self::FilmStandard,
        Self::FilmExtraShadow,
        Self::FilmHighContrast,
        Self::LinearResponse,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::FilmStandard => "Standard",
            Self::FilmExtraShadow => "Extra Shadow",
            Self::FilmHighContrast => "High Contrast",
            Self::LinearResponse => "Linear",
        }
    }

    pub const fn cli_slug(self) -> &'static str {
        match self {
            Self::FilmStandard => "film_standard",
            Self::FilmExtraShadow => "film_extra_shadow",
            Self::FilmHighContrast => "film_high_contrast",
            Self::LinearResponse => "linear_response",
        }
    }

    pub fn from_cli_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "film_standard" | "standard" => Some(Self::FilmStandard),
            "film_extra_shadow" | "extra_shadow" | "extra" => Some(Self::FilmExtraShadow),
            "film_high_contrast" | "high_contrast" | "contrast" => Some(Self::FilmHighContrast),
            "linear_response" | "linear" => Some(Self::LinearResponse),
            _ => None,
        }
    }

    #[inline]
    pub const fn to_uniform_f32(self) -> f32 {
        match self {
            Self::FilmStandard => 0.0,
            Self::FilmExtraShadow => 1.0,
            Self::FilmHighContrast => 2.0,
            Self::LinearResponse => 3.0,
        }
    }
}

/// Capture One-style Clarity local contrast algorithm selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClarityMethod {
    #[default]
    Natural,
    Punch,
    Neutral,
    Classic,
}

impl ClarityMethod {
    pub const ALL: [Self; 4] = [Self::Natural, Self::Punch, Self::Neutral, Self::Classic];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Natural => "Natural",
            Self::Punch => "Punch",
            Self::Neutral => "Neutral",
            Self::Classic => "Classic",
        }
    }

    pub const fn cli_slug(self) -> &'static str {
        match self {
            Self::Natural => "natural",
            Self::Punch => "punch",
            Self::Neutral => "neutral",
            Self::Classic => "classic",
        }
    }

    pub fn from_cli_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "natural" => Some(Self::Natural),
            "punch" => Some(Self::Punch),
            "neutral" => Some(Self::Neutral),
            "classic" => Some(Self::Classic),
            _ => None,
        }
    }

    #[inline]
    pub const fn to_uniform_f32(self) -> f32 {
        match self {
            Self::Natural => 0.0,
            Self::Punch => 1.0,
            Self::Neutral => 2.0,
            Self::Classic => 3.0,
        }
    }
}

/// Capture One-style photographic Film Grain emulsion profiles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrainType {
    #[default]
    Fine,
    SilverRich,
    Soft,
    Cubic,
    Tabular,
    Harsh,
}

impl GrainType {
    pub const ALL: [Self; 6] = [
        Self::Fine,
        Self::SilverRich,
        Self::Soft,
        Self::Cubic,
        Self::Tabular,
        Self::Harsh,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Fine => "Fine",
            Self::SilverRich => "Silver Rich",
            Self::Soft => "Soft",
            Self::Cubic => "Cubic",
            Self::Tabular => "Tabular",
            Self::Harsh => "Harsh",
        }
    }

    pub const fn cli_slug(self) -> &'static str {
        match self {
            Self::Fine => "fine",
            Self::SilverRich => "silver_rich",
            Self::Soft => "soft",
            Self::Cubic => "cubic",
            Self::Tabular => "tabular",
            Self::Harsh => "harsh",
        }
    }

    pub fn from_cli_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "fine" | "fine_grain" => Some(Self::Fine),
            "silver_rich" | "silver" => Some(Self::SilverRich),
            "soft" | "soft_grain" => Some(Self::Soft),
            "cubic" | "cubic_grain" => Some(Self::Cubic),
            "tabular" | "tabular_grain" => Some(Self::Tabular),
            "harsh" | "harsh_grain" => Some(Self::Harsh),
            _ => None,
        }
    }

    #[inline]
    pub const fn to_uniform_f32(self) -> f32 {
        match self {
            Self::Fine => 0.0,
            Self::SilverRich => 1.0,
            Self::Soft => 2.0,
            Self::Cubic => 3.0,
            Self::Tabular => 4.0,
            Self::Harsh => 5.0,
        }
    }
}

/// Capture One-style Crop-Aware Vignetting geometry method.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VignetteMethod {
    #[default]
    EllipticOnCrop,
    CircularOnCrop,
    Circular,
}

impl VignetteMethod {
    pub const ALL: [Self; 3] = [Self::EllipticOnCrop, Self::CircularOnCrop, Self::Circular];

    pub const fn label(self) -> &'static str {
        match self {
            Self::EllipticOnCrop => "Elliptic Crop",
            Self::CircularOnCrop => "Circular Crop",
            Self::Circular => "Circular",
        }
    }

    pub const fn cli_slug(self) -> &'static str {
        match self {
            Self::EllipticOnCrop => "elliptic_on_crop",
            Self::CircularOnCrop => "circular_on_crop",
            Self::Circular => "circular",
        }
    }

    pub fn from_cli_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "elliptic_on_crop" | "elliptic_crop" | "elliptic" => Some(Self::EllipticOnCrop),
            "circular_on_crop" | "circular_crop" => Some(Self::CircularOnCrop),
            "circular" | "circular_full" => Some(Self::Circular),
            _ => None,
        }
    }

    #[inline]
    pub const fn to_uniform_f32(self) -> f32 {
        match self {
            Self::EllipticOnCrop => 0.0,
            Self::CircularOnCrop => 1.0,
            Self::Circular => 2.0,
        }
    }
}

/// Non-destructive develop parameters for a single photograph.
///
/// All tone & color operations evaluate in **scene-referred linear light**
/// (16-bit half-float `Rgba16Float`), independent of display output gamut.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DevelopRecipe {
    pub schema_version: u32,
    /// Base Characteristics camera profile or film simulation ID (default `"auto"`).
    #[serde(default = "default_camera_profile")]
    pub camera_profile: String,
    /// Camera profile / film simulation blend strength (`0.0..=1.0`, default `1.0`).
    #[serde(default = "default_profile_strength")]
    pub profile_strength: f32,
    /// Base Characteristics Film Curve response (default `FilmStandard`).
    #[serde(default)]
    pub film_curve: FilmCurve,
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
    /// Active white balance illuminant preset (default `AsShot`).
    #[serde(default)]
    pub wb_preset: WbPreset,
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
    /// Crop-aware vignette falloff geometry (`EllipticOnCrop`, `CircularOnCrop`, `Circular`).
    #[serde(default)]
    pub vignette_method: VignetteMethod,
    /// Subtle photographic film grain amount (`0.0..=1.0`).
    pub grain: f32,
    /// Film grain emulsion profile (default `Fine`).
    #[serde(default)]
    pub grain_type: GrainType,
    /// Film grain granularity / clump size (`0.0..=1.0`, default `0.35`).
    #[serde(default = "default_grain_size")]
    pub grain_size: f32,
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
    /// Capture One Color Balance — Master global wheel.
    #[serde(default)]
    pub master_wheel: ColorWheel,
    /// RGB Combined, Master Luma, and per-channel R/G/B monotonic cubic Hermite tone curves.
    #[serde(default)]
    pub curves: ToneCurves,
    /// 8-Band HSL Color Editor (`Red`, `Orange`, `Yellow`, `Green`, `Aqua`, `Blue`, `Purple`, `Magenta`).
    #[serde(default)]
    pub hsl: [HslBand; HSL_BAND_COUNT],
    /// Capture One Advanced Color Editor (4 customizable hue/saturation range slices).
    #[serde(default)]
    pub advanced_color: [ColorEditorSlice; ADVANCED_COLOR_SLICE_COUNT],
    /// Capture One Skin Tone Uniformity & Amount controls.
    #[serde(default)]
    pub skin_tone: SkinToneParams,
    /// Local midtone contrast (`-1.0..=+1.0`).
    #[serde(default)]
    pub clarity: f32,
    /// Clarity algorithm method (`Natural`, `Punch`, `Neutral`, `Classic`).
    #[serde(default)]
    pub clarity_method: ClarityMethod,
    /// Unsharp mask sharpening amount (`0.0..=1.0`).
    #[serde(default)]
    pub sharpen: f32,
    /// Unsharp mask sharpening radius in pixels (`0.5..=4.0`, default `1.0`).
    #[serde(default = "default_one")]
    pub sharpen_radius: f32,
    /// Unsharp mask edge threshold (`0.0..=1.0`, default `0.0`).
    #[serde(default)]
    pub sharpen_threshold: f32,
    /// Unsharp mask halo suppression (`0.0..=1.0`, default `0.0`).
    #[serde(default)]
    pub sharpen_halo: f32,
    /// Edge-aware luminance / combined noise reduction (`0.0..=1.0`).
    #[serde(default)]
    pub noise_reduction: f32,
    /// Edge-aware color / chroma noise reduction (`0.0..=1.0`, default `0.0`).
    #[serde(default)]
    pub nr_color: f32,
    /// Capture One midtone-weighted luminance lift preserving black/white endpoints (`-1.0..=+1.0`).
    #[serde(default)]
    pub brightness: f32,
    /// Fine micro-contrast companion to Clarity (`-1.0..=+1.0`).
    #[serde(default)]
    pub structure: f32,
    /// Atmospheric contrast / airlight dehaze (`-1.0..=+1.0`).
    #[serde(default)]
    pub dehaze: f32,
    /// Capture One Master RGB + per-channel R/G/B Input/Output Levels controls.
    #[serde(default)]
    pub levels: LevelsParams,
    /// Capture One 6-Channel Black & White color sensitivity mixer + Split Toning.
    #[serde(default)]
    pub bw: BlackAndWhiteParams,
    /// Capture One Optical Lens Correction (distortion, chromatic aberration, purple fringing, light falloff).
    #[serde(default)]
    pub lens: LensParams,
    /// Persistent non-destructive Crop, Straighten, Rotation, Flip, and Keystone geometry.
    #[serde(default)]
    pub geometry: GeometryParams,
    /// Capture One-style local Adjustment Layers with composable masks (up to `MAX_ADJUSTMENT_LAYERS`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<AdjustmentLayer>,
    /// Spot Heal & Clone retouching stamps (up to `MAX_HEAL_SPOTS`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub heal_spots: Vec<HealSpot>,
    /// Base Characteristics processing mode (`Photography` default, or `FilmNegative`).
    #[serde(default)]
    pub processing_mode: ProcessingMode,
    /// Film Negative Conversion parameters (C-41 orange mask base, density, contrast, clips).
    #[serde(default)]
    pub film_negative: FilmNegativeParams,
}

impl Default for DevelopRecipe {
    fn default() -> Self {
        Self {
            schema_version: RECIPE_SCHEMA_VERSION,
            camera_profile: default_camera_profile(),
            profile_strength: default_profile_strength(),
            film_curve: FilmCurve::FilmStandard,
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            wb_preset: WbPreset::AsShot,
            temperature: 0.0,
            tint: 0.0,
            vibrance: 0.0,
            saturation: 0.0,
            vignette: 0.0,
            vignette_method: VignetteMethod::EllipticOnCrop,
            grain: 0.0,
            grain_type: GrainType::Fine,
            grain_size: default_grain_size(),
            preset_name: Some("Neutral".to_owned()),
            shadows_wheel: ColorWheel::default(),
            midtones_wheel: ColorWheel::default(),
            highlights_wheel: ColorWheel::default(),
            master_wheel: ColorWheel::default(),
            curves: ToneCurves::default(),
            hsl: [HslBand::default(); HSL_BAND_COUNT],
            advanced_color: [ColorEditorSlice::default(); ADVANCED_COLOR_SLICE_COUNT],
            skin_tone: SkinToneParams::default(),
            clarity: 0.0,
            clarity_method: ClarityMethod::Natural,
            sharpen: 0.0,
            sharpen_radius: default_one(),
            sharpen_threshold: 0.0,
            sharpen_halo: 0.0,
            noise_reduction: 0.0,
            nr_color: 0.0,
            brightness: 0.0,
            structure: 0.0,
            dehaze: 0.0,
            levels: LevelsParams::default(),
            bw: BlackAndWhiteParams::default(),
            lens: LensParams::default(),
            geometry: GeometryParams::default(),
            layers: Vec::new(),
            heal_spots: Vec::new(),
            processing_mode: ProcessingMode::Photography,
            film_negative: FilmNegativeParams::default(),
        }
    }
}

impl DevelopRecipe {
    pub fn is_neutral(&self) -> bool {
        let d = Self::default();
        self.camera_profile == d.camera_profile
            && (self.profile_strength - d.profile_strength).abs() <= 1e-6
            && self.film_curve == d.film_curve
            && self.exposure == d.exposure
            && self.contrast == d.contrast
            && self.highlights == d.highlights
            && self.shadows == d.shadows
            && self.whites == d.whites
            && self.blacks == d.blacks
            && self.wb_preset == d.wb_preset
            && self.temperature == d.temperature
            && self.tint == d.tint
            && self.vibrance == d.vibrance
            && self.saturation == d.saturation
            && self.vignette == d.vignette
            && self.vignette_method == d.vignette_method
            && self.grain == d.grain
            && self.grain_type == d.grain_type
            && (self.grain_size - d.grain_size).abs() <= 1e-6
            && self.shadows_wheel.is_neutral()
            && self.midtones_wheel.is_neutral()
            && self.highlights_wheel.is_neutral()
            && self.master_wheel.is_neutral()
            && self.curves.is_identity()
            && self.hsl.iter().all(HslBand::is_neutral)
            && self.advanced_color.iter().all(ColorEditorSlice::is_neutral)
            && self.skin_tone.is_neutral()
            && self.clarity == 0.0
            && self.clarity_method == d.clarity_method
            && self.sharpen == 0.0
            && (self.sharpen_radius - d.sharpen_radius).abs() <= 1e-6
            && self.sharpen_threshold == 0.0
            && self.sharpen_halo == 0.0
            && self.noise_reduction == 0.0
            && self.nr_color == 0.0
            && self.brightness == 0.0
            && self.structure == 0.0
            && self.dehaze == 0.0
            && self.levels.is_identity()
            && self.bw.is_neutral()
            && self.lens.is_identity()
            && self.geometry.is_identity()
            && self.layers.is_empty()
            && self.heal_spots.is_empty()
            && self.processing_mode == ProcessingMode::Photography
            && self.film_negative.is_default()
    }

    /// Samples a raw pre-grade linear RGB pixel from the film negative rebate border,
    /// switches `processing_mode` to [`ProcessingMode::FilmNegative`], and configures
    /// `film_negative.base_r/g/b`. Returns the sanitized `[base_r, base_g, base_b]`.
    pub fn convert_film_negative_from_base(&mut self, raw_linear_rgb: [f32; 3]) -> [f32; 3] {
        let base = sample_film_base_from_rgb(raw_linear_rgb);
        self.processing_mode = ProcessingMode::FilmNegative;
        self.film_negative.base_r = base[0];
        self.film_negative.base_g = base[1];
        self.film_negative.base_b = base[2];
        self.preset_name = None;
        base
    }

    /// Applies a [`WbPreset`], updating `wb_preset` and (for non-`Custom` presets)
    /// setting `temperature` and `tint` to the preset's standard illuminant values.
    pub fn apply_wb_preset(&mut self, preset: WbPreset) {
        self.wb_preset = preset;
        if let Some((temp, tint)) = wb_preset_values(preset) {
            self.temperature = temp;
            self.tint = tint;
        }
    }

    /// Samples a scene-referred linear RGB pixel to configure and enable Advanced Color Editor
    /// slice `slice_index` (`0..4`). Returns `Some((hue_deg, sat, lightness))` on success.
    pub fn pick_color_slice_from_rgb(
        &mut self,
        slice_index: usize,
        linear_rgb: [f32; 3],
    ) -> Option<(f32, f32, f32)> {
        let slot = self.advanced_color.get_mut(slice_index)?;
        let (h, s, v) = sample_color_slice_from_rgb(linear_rgb);
        slot.enabled = true;
        slot.target_hue = h;
        slot.target_sat = s;
        self.preset_name = None;
        Some((h, s, v))
    }

    /// Samples a scene-referred linear RGB pixel to configure and enable Skin Tone Uniformity.
    /// Returns `(hue_deg, sat, lightness)`.
    pub fn pick_skin_tone_from_rgb(&mut self, linear_rgb: [f32; 3]) -> (f32, f32, f32) {
        let (h, s, v) = sample_color_slice_from_rgb(linear_rgb);
        self.skin_tone.enabled = true;
        self.skin_tone.target_hue = h;
        self.skin_tone.target_sat = s;
        self.skin_tone.target_lightness = v;
        self.preset_name = None;
        (h, s, v)
    }

    /// Compiles this recipe into a fast per-pixel evaluator with a pre-baked 256-entry
    /// tone curve LUT and resolved camera profile 3D LUT for high-throughput histograms
    /// and full-resolution exports.
    pub fn compile(&self) -> CompiledRecipe {
        CompiledRecipe::from_recipe(self)
    }

    /// Compiles this recipe resolving `"user:<slug>"` camera profiles against `profiles_dir`
    /// (or [`default_profiles_dir`] when `None`).
    pub fn compile_with_profiles_dir(&self, profiles_dir: Option<&Path>) -> CompiledRecipe {
        CompiledRecipe::from_recipe_with_profiles_dir(self, profiles_dir)
    }

    /// Returns a sanitized copy of this recipe with all non-finite values zeroed and every
    /// parameter clamped to its canonical bounds from `registry::SCALAR_PARAMS`.
    pub fn sanitized(mut self) -> Self {
        for spec in SCALAR_PARAMS {
            let raw = (spec.get)(&self);
            let clean = if raw.is_finite() {
                raw.clamp(spec.min, spec.max)
            } else {
                spec.default
            };
            (spec.set)(&mut self, clean);
        }
        self.shadows_wheel = self.shadows_wheel.clamped();
        self.midtones_wheel = self.midtones_wheel.clamped();
        self.highlights_wheel = self.highlights_wheel.clamped();
        self.master_wheel = self.master_wheel.clamped();
        for band in &mut self.hsl {
            band.hue = if band.hue.is_finite() {
                band.hue.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            band.saturation = if band.saturation.is_finite() {
                band.saturation.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            band.luminance = if band.luminance.is_finite() {
                band.luminance.clamp(-1.0, 1.0)
            } else {
                0.0
            };
        }
        for slice in &mut self.advanced_color {
            *slice = slice.sanitized();
        }
        self.skin_tone = self.skin_tone.sanitized();
        self.levels = self.levels.sanitized();
        self.bw = self.bw.sanitized();
        self.lens = self.lens.sanitized();
        self.geometry.crop = self.geometry.sanitized_crop();
        self.geometry.straighten_deg = self.geometry.sanitized_straighten_deg();
        self.geometry.rotate_90 = self.geometry.quarter_turns() as i8;
        let [kv, kh, ka, k_amt] = self.geometry.sanitized_keystone();
        self.geometry.keystone_vertical = kv;
        self.geometry.keystone_horizontal = kh;
        self.geometry.keystone_aspect = ka;
        self.geometry.keystone_amount = k_amt;
        self.layers = self
            .layers
            .iter()
            .take(MAX_ADJUSTMENT_LAYERS)
            .map(AdjustmentLayer::sanitized)
            .collect();
        self.heal_spots = self
            .heal_spots
            .iter()
            .take(MAX_HEAL_SPOTS)
            .map(HealSpot::sanitized)
            .collect();
        self.film_negative = self.film_negative.sanitized();
        self.camera_profile = canonical_profile_id(&self.camera_profile);
        self
    }

    pub fn sidecar_path_for(photo_path: &Path) -> PathBuf {
        let mut s = photo_path.as_os_str().to_os_string();
        s.push(".rune-edit.json");
        PathBuf::from(s)
    }

    /// Saves this recipe into the photo's `.rune-edit.json` sidecar atomically,
    /// preserving any existing `PhotoMeta` rating, cull flag, revision, and provenance history.
    pub fn save_sidecar(&self, photo_path: &Path) -> Result<PathBuf, CoreError> {
        let mut meta = PhotoMeta::load_sidecar(photo_path)?.unwrap_or_default();
        meta.recipe = self.clone().sanitized();
        meta.save_sidecar(photo_path)
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

const fn default_loupe_uv() -> [f32; 2] {
    [0.5, 0.5]
}

const fn default_loupe_zoom() -> f32 {
    2.5
}

const fn default_loupe_radius_px() -> f32 {
    96.0
}

const fn default_view_color_range() -> i32 {
    -1
}

const fn default_split_pos() -> f32 {
    0.5
}

/// Interactive camera/viewport transform state for the central darkroom canvas.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewportCamera {
    /// Zoom multiplier relative to fit-to-viewport (`1.0` = Fit, `0.25..=16.0`).
    #[serde(default = "default_one")]
    pub zoom: f32,
    /// Pan offset in normalized viewport units (`[-2.0, 2.0]`).
    #[serde(default)]
    pub pan: [f32; 2],
    /// View rotation in degrees (`-180.0..=180.0`).
    #[serde(default)]
    pub rotation_deg: f32,
    /// Whether Before/After comparison is enabled.
    #[serde(default)]
    pub split_compare: bool,
    /// Horizontal split divider position in `[0.0, 1.0]` (`0.5` = center).
    #[serde(default = "default_split_pos")]
    pub split_pos: f32,
    /// Momentary full-canvas "Before" preview (hold `\`).
    #[serde(default)]
    pub show_original: bool,
    /// Highlight (red) & shadow (blue) clipping diagnostic overlay.
    #[serde(default)]
    pub exposure_warning: bool,
    /// Real-time green acutance focus mask overlay.
    #[serde(default)]
    pub focus_mask: bool,
    /// Interactive crop framing mode (shows full uncropped frame with dimmed outer mask & grid).
    #[serde(default)]
    pub crop_mode: bool,
    /// Transient `View Selected Color Range` mask preview (`-1` = off, `0..=3` = Advanced slice, `4` = Skin Tone).
    #[serde(default = "default_view_color_range")]
    pub view_color_range: i32,
    /// Enables the 120Hz circular GPU Loupe magnifier overlay.
    #[serde(default)]
    pub loupe_enabled: bool,
    /// Loupe center in viewport slot UV coordinates (`[0.0, 1.0]^2`).
    #[serde(default = "default_loupe_uv")]
    pub loupe_uv: [f32; 2],
    /// Optical magnification factor inside the Loupe lens (`1.25..=16.0`, default `2.5`).
    #[serde(default = "default_loupe_zoom")]
    pub loupe_zoom: f32,
    /// Loupe circle radius in viewport device pixels (`24.0..=320.0`, default `96.0`).
    #[serde(default = "default_loupe_radius_px")]
    pub loupe_radius_px: f32,
    /// Mask overlay display mode (`0` = Off, `1` = Red Rubylith overlay, `2` = Grayscale mask).
    #[serde(default)]
    pub mask_overlay_mode: i32,
    /// Active adjustment layer index for mask overlay visualization (`-1` = none, `0..8`).
    #[serde(default = "default_view_color_range")]
    pub active_layer_index: i32,
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
            exposure_warning: false,
            focus_mask: false,
            crop_mode: false,
            view_color_range: -1,
            loupe_enabled: false,
            loupe_uv: default_loupe_uv(),
            loupe_zoom: default_loupe_zoom(),
            loupe_radius_px: default_loupe_radius_px(),
            mask_overlay_mode: 0,
            active_layer_index: -1,
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
                wb_preset: WbPreset::Custom,
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
                wb_preset: WbPreset::Custom,
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
                wb_preset: WbPreset::Custom,
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
                wb_preset: WbPreset::Custom,
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
                wb_preset: WbPreset::Custom,
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
    pub profile_lut: Option<Arc<ProfileLut3D>>,
    pub profile_strength: f32,
    pub profile_active: bool,
    pub film_curve: FilmCurve,
    pub wb_exp: [f32; 3],
    pub brightness: f32,
    pub contrast_pow: f32,
    pub contrast_active: bool,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub tones_active: bool,
    pub levels: LevelsParams,
    pub levels_active: bool,
    pub dehaze: f32,
    pub vibrance: f32,
    pub saturation: f32,
    pub sat_active: bool,
    pub shadows_wheel: ColorWheel,
    pub midtones_wheel: ColorWheel,
    pub highlights_wheel: ColorWheel,
    pub master_wheel: ColorWheel,
    pub wheels_active: bool,
    pub curve_lut: [[f32; 4]; CURVE_LUT_SIZE],
    pub curves_active: bool,
    pub hsl: [HslBand; HSL_BAND_COUNT],
    pub hsl_active: bool,
    pub advanced_color: [ColorEditorSlice; ADVANCED_COLOR_SLICE_COUNT],
    pub advanced_color_active: bool,
    pub skin_tone: SkinToneParams,
    pub skin_tone_active: bool,
    pub bw_enabled: bool,
    pub bw_channels: [f32; 6],
    pub bw_split: [f32; 4],
    pub processing_mode: ProcessingMode,
    pub film_negative: FilmNegativeParams,
}

impl CompiledRecipe {
    pub fn from_recipe(recipe: &DevelopRecipe) -> Self {
        Self::from_recipe_with_profiles_dir(recipe, None)
    }

    pub fn from_recipe_with_profiles_dir(
        recipe: &DevelopRecipe,
        profiles_dir: Option<&Path>,
    ) -> Self {
        let wb_r = 2.0_f32.powf(recipe.temperature * 0.45 + recipe.tint * 0.22);
        let wb_g = 2.0_f32.powf(-recipe.tint * 0.35);
        let wb_b = 2.0_f32.powf(-recipe.temperature * 0.45 + recipe.tint * 0.22);
        let exp_gain = 2.0_f32.powf(recipe.exposure);

        let brightness = if recipe.brightness.is_finite() {
            recipe.brightness.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let dehaze = if recipe.dehaze.is_finite() {
            recipe.dehaze.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let levels = recipe.levels.sanitized();
        let levels_active = !levels.is_identity();

        let shadows_wheel = recipe.shadows_wheel.clamped();
        let midtones_wheel = recipe.midtones_wheel.clamped();
        let highlights_wheel = recipe.highlights_wheel.clamped();
        let master_wheel = recipe.master_wheel.clamped();
        let wheels_active = !shadows_wheel.is_neutral()
            || !midtones_wheel.is_neutral()
            || !highlights_wheel.is_neutral()
            || !master_wheel.is_neutral();

        let curves_active = !recipe.curves.is_identity();
        let curve_lut = recipe.curves.bake_curve_lut_f32();
        let hsl_active = recipe.hsl.iter().any(|b| !b.is_neutral());
        let mut advanced_color = recipe.advanced_color;
        for s in &mut advanced_color {
            *s = s.sanitized();
        }
        let advanced_color_active = advanced_color.iter().any(ColorEditorSlice::is_active);
        let skin_tone = recipe.skin_tone.sanitized();
        let skin_tone_active = skin_tone.is_active();
        let bw_enabled = recipe.bw.enabled;
        let bw_channels = recipe.bw.channels_clamped();
        let bw_split = recipe.bw.split_tones_clamped();

        let contrast_pow = 2.0_f32.powf(recipe.contrast * 0.85);
        let contrast_active = recipe.contrast.abs() > 1e-6;
        let tones_active = recipe.shadows.abs() > 1e-6
            || recipe.highlights.abs() > 1e-6
            || recipe.blacks.abs() > 1e-6
            || recipe.whites.abs() > 1e-6
            || brightness.abs() > 1e-6;
        let sat_active = recipe.saturation.abs() > 1e-6 || recipe.vibrance.abs() > 1e-6;
        let profile_strength = if recipe.profile_strength.is_finite() {
            recipe.profile_strength.clamp(0.0, 1.0)
        } else {
            default_profile_strength()
        };
        let profile_lut = if profile_strength > 1e-5 {
            resolve_profile_lut(&recipe.camera_profile, profiles_dir)
        } else {
            None
        };
        let profile_active = profile_lut.is_some();

        Self {
            profile_lut,
            profile_strength,
            profile_active,
            film_curve: recipe.film_curve,
            wb_exp: [wb_r * exp_gain, wb_g * exp_gain, wb_b * exp_gain],
            brightness,
            contrast_pow,
            contrast_active,
            highlights: recipe.highlights,
            shadows: recipe.shadows,
            whites: recipe.whites,
            blacks: recipe.blacks,
            tones_active,
            levels,
            levels_active,
            dehaze,
            vibrance: recipe.vibrance,
            saturation: recipe.saturation,
            sat_active,
            shadows_wheel,
            midtones_wheel,
            highlights_wheel,
            master_wheel,
            wheels_active,
            curve_lut,
            curves_active,
            hsl: recipe.hsl,
            hsl_active,
            advanced_color,
            advanced_color_active,
            skin_tone,
            skin_tone_active,
            bw_enabled,
            bw_channels,
            bw_split,
            processing_mode: recipe.processing_mode,
            film_negative: recipe.film_negative.sanitized(),
        }
    }

    /// Evaluates Stages 0.5..=8 (Film Negative inversion, White Balance, Exposure, Tones,
    /// Contrast, Levels, Dehaze, Tone Curves, 3-Way + Master Color Wheels, and 8-Band HSL)
    /// in scene-referred linear light right *before* Stage 8b/8c (Advanced Color & Skin Tone).
    ///
    /// Use this for Color Slice / Skin Tone eyedroppers so picked targets match the exact
    /// color space evaluated by `advanced_slice_mask`, `skin_tone_mask`, and `View Selected Color Range`.
    #[inline]
    pub fn evaluate_pre_advanced_color_linear(&self, rgb: [f32; 3]) -> [f32; 3] {
        // 0.5. Optional Film Negative optical inversion (zero cost when Photography mode)
        let rgb = if self.processing_mode == ProcessingMode::FilmNegative {
            invert_film_negative_linear(rgb, &self.film_negative)
        } else {
            rgb
        };

        // 0.75. Optional Camera Profile / Film Simulation 33^3 3D LUT (zero cost when inactive)
        let rgb = if let Some(lut) = &self.profile_lut {
            let c = [rgb[0].max(0.0), rgb[1].max(0.0), rgb[2].max(0.0)];
            let mapped = lut.sample_trilinear(c);
            let s = self.profile_strength;
            [
                (c[0] + (mapped[0] - c[0]) * s).max(0.0),
                (c[1] + (mapped[1] - c[1]) * s).max(0.0),
                (c[2] + (mapped[2] - c[2]) * s).max(0.0),
            ]
        } else {
            rgb
        };

        // 1. White balance + Exposure (linear stops)
        let mut r = (rgb[0] * self.wb_exp[0]).max(0.0);
        let mut g = (rgb[1] * self.wb_exp[1]).max(0.0);
        let mut b = (rgb[2] * self.wb_exp[2]).max(0.0);

        // 2. Tonal regions (Shadows / Highlights / Whites / Blacks / Midtone Brightness)
        if self.tones_active {
            let luma = (0.2126 * r + 0.7152 * g + 0.0722 * b).max(1e-5);
            let t = (luma / (luma + 0.18)).clamp(0.0, 1.0);
            let shadow_w = (1.0 - t) * (1.0 - t);
            let highlight_w = t * t;
            let black_w = shadow_w * shadow_w;
            let white_w = highlight_w * highlight_w;
            let mid_w = 4.0 * t * (1.0 - t);

            let tone_stops = self.shadows * 1.15 * shadow_w
                + self.highlights * 1.15 * highlight_w
                + self.blacks * 0.90 * black_w
                + self.whites * 0.75 * white_w
                + self.brightness * 0.95 * mid_w;
            let tone_gain = 2.0_f32.powf(tone_stops);
            r *= tone_gain;
            g *= tone_gain;
            b *= tone_gain;
        }

        // 3. Contrast around 18% middle gray pivot
        if self.contrast_active {
            let pivot = 0.18_f32;
            r = pivot * (r / pivot).max(0.0).powf(self.contrast_pow);
            g = pivot * (g / pivot).max(0.0).powf(self.contrast_pow);
            b = pivot * (b / pivot).max(0.0).powf(self.contrast_pow);
        }

        // 4. Levels (Input black/mid/white + Output black/white)
        if self.levels_active {
            [r, g, b] = crate::color_science::apply_levels([r, g, b], self.levels);
        }

        // 5. Atmospheric Dehaze
        if self.dehaze.abs() > 1e-6 {
            [r, g, b] = crate::color_science::apply_dehaze([r, g, b], self.dehaze);
        }

        // 6. Tone Curves (Luma + per-channel R/G/B in perceptual gamma 2.2 space, HDR-preserving)
        if self.curves_active {
            [r, g, b] = apply_tone_curves_lut([r, g, b], &self.curve_lut);
        }

        // 7. Capture One Color Balance (3-Way Shadows / Midtones / Highlights + Master wheel)
        if self.wheels_active {
            [r, g, b] = apply_color_wheels(
                [r, g, b],
                self.shadows_wheel,
                self.midtones_wheel,
                self.highlights_wheel,
                self.master_wheel,
            );
        }

        // 8. 8-Band HSL Color Editor
        if self.hsl_active {
            [r, g, b] = apply_hsl_bands([r, g, b], &self.hsl);
        }

        [r, g, b]
    }

    /// Evaluates all per-pixel color & tone stages in scene-referred linear light *before*
    /// optical vignette, film grain, and filmic highlight roll-off (matching `shaders/develop.wgsl`).
    ///
    /// Use this in full-resolution export when applying vignette/grain pre-tonemap, then
    /// finish with [`filmic_tonemap_with_curve`].
    #[inline]
    pub fn evaluate_pixel_scene_linear(&self, rgb: [f32; 3]) -> [f32; 3] {
        let [mut r, mut g, mut b] = self.evaluate_pre_advanced_color_linear(rgb);

        // 8b. Advanced Color Editor (4 customizable range slices)
        if self.advanced_color_active {
            [r, g, b] = crate::color_science::apply_advanced_color([r, g, b], &self.advanced_color);
        }

        // 8c. Skin Tone Uniformity & Amount
        if self.skin_tone_active {
            [r, g, b] = crate::color_science::apply_skin_tone([r, g, b], self.skin_tone);
        }

        // 9. Vibrance & Saturation
        if self.sat_active {
            let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let max_c = r.max(g).max(b);
            let min_c = r.min(g).min(b);
            let est_sat = if max_c > 1e-5 {
                ((max_c - min_c) / max_c).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let sat_scale =
                (1.0 + self.saturation + self.vibrance * (1.0 - est_sat) * 0.75).max(0.0);
            r = (y + (r - y) * sat_scale).max(0.0);
            g = (y + (g - y) * sat_scale).max(0.0);
            b = (y + (b - y) * sat_scale).max(0.0);
        }

        // 10. 6-Channel Black & White Mixer + Split Toning
        if self.bw_enabled {
            [r, g, b] =
                crate::color_science::apply_bw_mixer([r, g, b], self.bw_channels, self.bw_split);
        }

        [r, g, b]
    }

    /// Evaluates a single scene-referred linear RGB pixel, matching `shaders/develop.wgsl`
    /// across WB, Exposure, Tones, Contrast, Tone Curves, 3-Way + Master Color Balance,
    /// 8-Band HSL, Vibrance/Saturation, B&W + Split Toning, and Base Characteristics film curve.
    #[inline]
    pub fn evaluate_pixel_linear(&self, rgb: [f32; 3]) -> [f32; 3] {
        filmic_tonemap_with_curve(self.evaluate_pixel_scene_linear(rgb), self.film_curve)
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
    master: ColorWheel,
) -> [f32; 3] {
    let luma = (0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]).max(1e-5);
    let t = (luma / (luma + 0.18)).clamp(0.0, 1.0);
    let w_s = (1.0 - t) * (1.0 - t);
    let w_m = 2.0 * t * (1.0 - t);
    let w_h = t * t;

    let ts = wheel_tint_rgb(shadows);
    let tm = wheel_tint_rgb(midtones);
    let th = wheel_tint_rgb(highlights);
    let tg = wheel_tint_rgb(master);

    let luma_shift = shadows.luma * w_s + midtones.luma * w_m + highlights.luma * w_h + master.luma;
    let shadow_lift = w_s * w_s * 0.025;

    let mut out = [0.0_f32; 3];
    for i in 0..3 {
        let color_shift = ts[i] * w_s + tm[i] * w_m + th[i] * w_h + tg[i];
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
pub(crate) fn rgb_to_hsv(rgb: [f32; 3]) -> [f32; 3] {
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
pub(crate) fn hsv_to_rgb(hsv: [f32; 3]) -> [f32; 3] {
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

/// Soft filmic highlight roll-off (`FilmCurve::FilmStandard`) matching `filmic_tonemap`
/// in `shaders/develop.wgsl`.
///
/// Leaves midtones (`<= 0.75` linear) untouched and smoothly compresses `> 0.75` HDR highlights
/// into `(0.75, 1.0)`.
#[inline]
pub fn filmic_tonemap(rgb: [f32; 3]) -> [f32; 3] {
    filmic_tonemap_with_curve(rgb, FilmCurve::FilmStandard)
}

/// Evaluates the Base Characteristics [`FilmCurve`] tone map in linear light,
/// matching `filmic_tonemap_curve` in `shaders/develop.wgsl`.
#[inline]
pub fn filmic_tonemap_with_curve(rgb: [f32; 3], curve: FilmCurve) -> [f32; 3] {
    [
        filmic_tonemap_channel_curve(rgb[0], curve),
        filmic_tonemap_channel_curve(rgb[1], curve),
        filmic_tonemap_channel_curve(rgb[2], curve),
    ]
}

#[inline]
fn filmic_tonemap_channel_curve(x: f32, curve: FilmCurve) -> f32 {
    let v = x.max(0.0);
    match curve {
        FilmCurve::FilmStandard => {
            // Gentle shoulder starting above ~0.75 so midtones stay faithful and >1.0 HDR rolls off smoothly.
            if v <= 0.75 {
                v
            } else {
                let over = v - 0.75;
                0.75 + (over / (1.0 + over * 4.0))
            }
        }
        FilmCurve::FilmExtraShadow => {
            // Smooth shadow lift preserving black point (0 -> 0) with gentle highlight shoulder at 0.70.
            let lifted = v * (1.0 + 0.55 / (1.0 + v * 7.0));
            if lifted <= 0.70 {
                lifted
            } else {
                let over = lifted - 0.70;
                0.70 + (over / (1.0 + over * (1.0 / 0.30)))
            }
        }
        FilmCurve::FilmHighContrast => {
            // S-curve contrast around 18% middle gray with crisp filmic highlight shoulder at 0.72.
            if v <= 0.0 {
                return 0.0;
            }
            let c = 0.18 * (v / 0.18).powf(1.22);
            if c <= 0.72 {
                c
            } else {
                let over = c - 0.72;
                0.72 + (over / (1.0 + over * (1.0 / 0.28)))
            }
        }
        FilmCurve::LinearResponse => {
            // Scientific / reproduction linear response with display [0, 1] clamp.
            v.clamp(0.0, 1.0)
        }
    }
}

const LINEAR_TO_SRGB_LUT_SIZE: usize = 4096;

#[inline]
fn linear_to_srgb_lut() -> &'static [f32; LINEAR_TO_SRGB_LUT_SIZE] {
    static LUT: OnceLock<[f32; LINEAR_TO_SRGB_LUT_SIZE]> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut table = [0.0_f32; LINEAR_TO_SRGB_LUT_SIZE];
        let denom = (LINEAR_TO_SRGB_LUT_SIZE - 1) as f32;
        for (i, slot) in table.iter_mut().enumerate() {
            let c = i as f32 / denom;
            *slot = if c <= 0.0031308 {
                12.92 * c
            } else {
                1.055 * c.powf(1.0 / 2.4) - 0.055
            };
        }
        table[0] = 0.0;
        table[LINEAR_TO_SRGB_LUT_SIZE - 1] = 1.0;
        table
    })
}

/// Fast IEC 61966-2-1 linear-to-sRGB transfer using the exact linear toe below `0.0031308`
/// and a linearly-interpolated 4096-entry LUT above it (max error `< 2e-5` vs exact `powf`).
#[inline]
pub fn linear_to_srgb(x: f32) -> f32 {
    if !x.is_finite() || x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    if x <= 0.0031308 {
        return 12.92 * x;
    }
    let scaled = x * (LINEAR_TO_SRGB_LUT_SIZE - 1) as f32;
    let idx = scaled as usize;
    if idx >= LINEAR_TO_SRGB_LUT_SIZE - 1 {
        return 1.0;
    }
    let frac = scaled - idx as f32;
    let lut = linear_to_srgb_lut();
    lut[idx] + (lut[idx + 1] - lut[idx]) * frac
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

/// Computes a fast 64-bin RGB + Luma histogram from a downsampled linear RGB proxy
/// using an already-compiled recipe (zero redundant tone-curve LUT baking on 120Hz drags).
pub fn compute_histogram_compiled(
    proxy_linear_rgb: &[[f32; 3]],
    compiled: &CompiledRecipe,
) -> HistogramData {
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

/// Computes a fast 64-bin RGB + Luma histogram from a downsampled linear RGB proxy.
pub fn compute_histogram(proxy_linear_rgb: &[[f32; 3]], recipe: &DevelopRecipe) -> HistogramData {
    compute_histogram_compiled(proxy_linear_rgb, &CompiledRecipe::from_recipe(recipe))
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Validation error: {0}")]
    Validation(String),
    #[error("Unknown parameter '{0}'")]
    UnknownParam(String),
    #[error("Unknown preset '{0}'")]
    UnknownPreset(String),
    #[error("Sidecar conflict: expected revision {expected}, found {actual}")]
    SidecarConflict { expected: u64, actual: u64 },
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
            if preset.recipe.temperature.abs() > 1e-5 || preset.recipe.tint.abs() > 1e-5 {
                assert_ne!(
                    preset.recipe.wb_preset,
                    WbPreset::AsShot,
                    "Preset '{}' shifts WB so wb_preset must not be AsShot",
                    preset.name
                );
            }
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
        assert_eq!(parsed_recipe.brightness, 0.0);
        assert_eq!(parsed_recipe.structure, 0.0);
        assert_eq!(parsed_recipe.dehaze, 0.0);
        assert!(parsed_recipe.levels.is_identity());
        assert!(parsed_recipe.bw.is_neutral());
        assert!(parsed_recipe.geometry.is_identity());
        assert_eq!(parsed_recipe.film_curve, FilmCurve::FilmStandard);
        assert_eq!(parsed_recipe.camera_profile, "auto");
        assert!((parsed_recipe.profile_strength - 1.0).abs() < 1e-6);
        assert_eq!(parsed_recipe.wb_preset, WbPreset::AsShot);
        assert_eq!(parsed_recipe.clarity_method, ClarityMethod::Natural);
        assert_eq!(parsed_recipe.grain_type, GrainType::Fine);
        assert!((parsed_recipe.grain_size - 0.35).abs() < 1e-6);
        assert!((parsed_recipe.sharpen_radius - 1.0).abs() < 1e-6);
        assert_eq!(parsed_recipe.sharpen_threshold, 0.0);
        assert_eq!(parsed_recipe.sharpen_halo, 0.0);
        assert_eq!(parsed_recipe.nr_color, 0.0);
        assert!(parsed_recipe.master_wheel.is_neutral());

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
            ..PhotoMeta::default()
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
        assert_eq!(loaded_meta.rating, meta.rating);
        assert_eq!(loaded_meta.flag, meta.flag);
        assert_eq!(loaded_meta.recipe, meta.recipe);
        assert_eq!(loaded_meta.revision, meta.revision + 1);
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

    #[test]
    fn brightness_midtone_lift_preserves_endpoints() {
        let recipe = DevelopRecipe {
            brightness: 0.8,
            ..DevelopRecipe::default()
        };
        let compiled = recipe.compile();
        let black = compiled.evaluate_pixel_scene_linear([0.0, 0.0, 0.0]);
        assert_eq!(black, [0.0, 0.0, 0.0]);
        let mid = compiled.evaluate_pixel_scene_linear([0.18, 0.18, 0.18]);
        assert!(mid[0] > 0.18 * 1.5, "midtones should lift, got {mid:?}");
        let hi = compiled.evaluate_pixel_scene_linear([4.0, 4.0, 4.0]);
        let mid_ratio = mid[0] / 0.18;
        let hi_ratio = hi[0] / 4.0;
        assert!(
            mid_ratio > hi_ratio * 1.4,
            "midtones ({mid_ratio}) should lift far more than highlights ({hi_ratio})"
        );
    }

    #[test]
    fn levels_identity_contrast_and_adversarial_sanitization() {
        let id = LevelsParams::default();
        assert!(id.is_identity());
        let id_recipe = DevelopRecipe::default().compile();
        let hdr = [1.5, 0.5, 0.05];
        let out = id_recipe.evaluate_pixel_scene_linear(hdr);
        for ch in 0..3 {
            assert!((out[ch] - hdr[ch]).abs() < 1e-6);
        }

        // Input black/white stretch + midtone lift
        let stretched = DevelopRecipe {
            levels: LevelsParams {
                in_black: 0.10,
                in_mid: 0.35,
                in_white: 0.85,
                out_black: 0.02,
                out_white: 0.96,
                ..LevelsParams::default()
            },
            ..DevelopRecipe::default()
        }
        .compile();
        let mid = stretched.evaluate_pixel_scene_linear([0.18, 0.18, 0.18]);
        assert!(mid[0] > 0.18 && mid[0].is_finite());

        // Adversarial inverted / NaN / Inf levels must sanitize cleanly without NaN or Inf.
        let bad = LevelsParams {
            in_black: 0.95,
            in_mid: f32::NAN,
            in_white: 0.10,
            out_black: f32::INFINITY,
            out_white: f32::NEG_INFINITY,
            ..LevelsParams::default()
        };
        let clean = bad.sanitized();
        assert!(clean.in_white - clean.in_black >= 0.01 - 1e-6);
        let bad_recipe = DevelopRecipe {
            levels: bad,
            ..DevelopRecipe::default()
        }
        .compile();
        let res = bad_recipe.evaluate_pixel_scene_linear([0.2, 0.5, 0.8]);
        assert!(res.iter().all(|v| v.is_finite() && *v >= 0.0));
    }

    #[test]
    fn dehaze_positive_negative_and_edge_cases() {
        let hazy = [0.22, 0.24, 0.28];
        let pos = DevelopRecipe {
            dehaze: 0.7,
            ..DevelopRecipe::default()
        }
        .compile()
        .evaluate_pixel_scene_linear(hazy);
        let neg = DevelopRecipe {
            dehaze: -0.7,
            ..DevelopRecipe::default()
        }
        .compile()
        .evaluate_pixel_scene_linear(hazy);
        // Positive dehaze deepens chroma separation; negative lifts shadows toward airlight.
        assert!((pos[2] - pos[0]) > (hazy[2] - hazy[0]));
        assert!(neg[0] > hazy[0]);

        // Pure black and NaN stay safe & finite.
        let black = DevelopRecipe {
            dehaze: 1.0,
            ..DevelopRecipe::default()
        }
        .compile()
        .evaluate_pixel_scene_linear([0.0, 0.0, 0.0]);
        assert_eq!(black, [0.0, 0.0, 0.0]);
        let nan_out = DevelopRecipe {
            dehaze: f32::NAN,
            ..DevelopRecipe::default()
        }
        .compile()
        .evaluate_pixel_scene_linear(hazy);
        for ch in 0..3 {
            assert!((nan_out[ch] - hazy[ch]).abs() < 1e-6);
        }
    }

    #[test]
    fn six_channel_bw_selectivity_and_neutral_invariance() {
        let bw_recipe = DevelopRecipe {
            bw: BlackAndWhiteParams {
                enabled: true,
                red: 0.7,
                blue: -0.8,
                ..BlackAndWhiteParams::default()
            },
            ..DevelopRecipe::default()
        }
        .compile();

        let red_px = [0.45, 0.03, 0.03];
        let blue_px = [0.03, 0.03, 0.45];
        let gray_px = [0.18, 0.18, 0.18];

        let r_out = bw_recipe.evaluate_pixel_scene_linear(red_px);
        let b_out = bw_recipe.evaluate_pixel_scene_linear(blue_px);
        let g_out = bw_recipe.evaluate_pixel_scene_linear(gray_px);

        // Strictly monochrome outputs
        assert!((r_out[0] - r_out[1]).abs() < 1e-6 && (r_out[1] - r_out[2]).abs() < 1e-6);
        assert!((b_out[0] - b_out[1]).abs() < 1e-6 && (b_out[1] - b_out[2]).abs() < 1e-6);
        // Neutral gray invariant to color channel offsets
        assert!((g_out[0] - 0.18).abs() < 1e-5);
        // Red lifted above its Rec.709 luma, Blue darkened below its Rec.709 luma
        let red_base = 0.2126 * red_px[0] + 0.7152 * red_px[1] + 0.0722 * red_px[2];
        let blue_base = 0.2126 * blue_px[0] + 0.7152 * blue_px[1] + 0.0722 * blue_px[2];
        assert!(r_out[0] > red_base * 1.4);
        assert!(b_out[0] < blue_base * 0.65);
    }

    #[test]
    fn auto_and_eyedropper_color_science() {
        // Pick WB neutralizes a warm/green cast accurately
        let cast = [0.24_f32, 0.20, 0.16];
        let (temp, tint) = pick_white_balance(cast);
        let wb_recipe = DevelopRecipe {
            temperature: temp,
            tint,
            ..DevelopRecipe::default()
        }
        .compile();
        let neutralized = wb_recipe.evaluate_pixel_scene_linear(cast);
        assert!((neutralized[0] - neutralized[1]).abs() < 1e-4);
        assert!((neutralized[1] - neutralized[2]).abs() < 1e-4);

        // Adversarial black & NaN inputs must return (0, 0) safely
        assert_eq!(pick_white_balance([0.0, 0.0, 0.0]), (0.0, 0.0));
        assert_eq!(pick_white_balance([f32::NAN, 0.2, 0.2]), (0.0, 0.0));

        // Auto WB recovers uniform warm cast
        let warm_proxy = vec![cast; 64];
        let (auto_t, auto_ti) = auto_white_balance(&warm_proxy);
        assert!((auto_t - temp).abs() < 1e-4 && (auto_ti - tint).abs() < 1e-4);

        // Auto exposure lifts dark proxy toward 18% gray
        let dark_proxy = vec![[0.045, 0.045, 0.045]; 64];
        let (exp, _con, _hi, _sh) = auto_exposure_and_tone(&dark_proxy);
        assert!(exp > 1.0 && exp.is_finite());

        // Auto levels finds 0.1% shadow/highlight bounds
        let mut ramp = Vec::with_capacity(2000);
        for i in 0..2000 {
            let t = 0.15 + 0.65 * (i as f32 / 1999.0);
            let lin = t.powf(2.2);
            ramp.push([lin, lin, lin]);
        }
        let lv = auto_levels(&ramp);
        assert!((lv.in_black - 0.15).abs() < 0.015, "got {}", lv.in_black);
        assert!((lv.in_white - 0.80).abs() < 0.015, "got {}", lv.in_white);

        // RGB -> HSL & CIE Lab (D65) sanity checks
        let (hsl_w, lab_w) = rgb_to_hsl_and_lab([1.0, 1.0, 1.0]);
        assert!((hsl_w[2] - 100.0).abs() < 1e-3 && (lab_w[0] - 100.0).abs() < 1e-2);
        assert!(lab_w[1].abs() < 0.05 && lab_w[2].abs() < 0.05);

        let (hsl_r, lab_r) = rgb_to_hsl_and_lab([1.0, 0.0, 0.0]);
        assert!((hsl_r[0] - 0.0).abs() < 1e-3 && (hsl_r[1] - 100.0).abs() < 1e-3);
        assert!((lab_r[0] - 53.23).abs() < 0.15 && (lab_r[1] - 80.11).abs() < 0.2);

        let (hsl_nan, lab_nan) = rgb_to_hsl_and_lab([f32::NAN, -1.0, 2.0]);
        assert!(hsl_nan.iter().all(|v| v.is_finite()));
        assert!(lab_nan.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn geometry_transforms_aspect_presets_and_sanitization() {
        let mut geo = GeometryParams {
            crop: [0.95, 0.95, 0.50, -0.20],
            straighten_deg: 90.0,
            rotate_90: 1,
            flip_h: true,
            flip_v: false,
            aspect_preset: CropAspect::Free,
            grid: CompositionGrid::GoldenRatio,
            ..GeometryParams::default()
        };
        let c = geo.sanitized_crop();
        assert!(c[0] + c[2] <= 1.0 + 1e-6 && c[2] >= 0.01);
        assert!(c[1] + c[3] <= 1.0 + 1e-6 && c[3] >= 0.01);
        assert_eq!(geo.sanitized_straighten_deg(), 45.0);
        assert_eq!(geo.oriented_size(1600, 1000), [1000, 1600]);

        // 90 deg CW: oriented top-right (1, 0) comes from source top-left (0, 0)
        geo.straighten_deg = 0.0;
        geo.flip_h = false;
        let su = geo
            .oriented_to_source_uv(1.0, 0.0, 1600, 1000)
            .expect("valid uv");
        assert!((su[0] - 0.0).abs() < 1e-5 && (su[1] - 0.0).abs() < 1e-5);

        // Square aspect preset produces equal pixel width & height
        let mut sq = GeometryParams::default();
        sq.apply_aspect_preset(CropAspect::Square, 1600, 1000);
        let [ow, oh] = sq.cropped_output_size(1600, 1000);
        assert_eq!(ow, oh);
        assert_eq!(ow, 1000);

        // viewport_to_oriented_uv center maps to (0.5, 0.5) in crop_mode
        let crop_cam = ViewportCamera {
            crop_mode: true,
            ..ViewportCamera::default()
        };
        let ori_center =
            sq.viewport_to_oriented_uv([0.5, 0.5], [800.0, 600.0], [1600, 1000], &crop_cam);
        assert!((ori_center[0] - 0.5).abs() < 1e-5 && (ori_center[1] - 0.5).abs() < 1e-5);

        // Non-finite ColorWheel::clamped sanitizes cleanly to 0.0
        let bad_wheel = ColorWheel::new(f32::NAN, f32::INFINITY, f32::NEG_INFINITY).clamped();
        assert!(bad_wheel.is_neutral());
    }

    #[test]
    fn save_sidecar_in_place_advances_revision_and_checks_conflicts() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("rune_core_rev_test_{}_{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let photo = dir.join("frame.dng");
        std::fs::write(&photo, b"").expect("create dummy photo");

        let mut meta = PhotoMeta::default();
        assert_eq!(meta.revision, 0);

        meta.rating = 4;
        let sc_path = meta
            .save_sidecar_in_place(&photo)
            .expect("first save_sidecar_in_place");
        assert_eq!(meta.revision, 1);

        let disk1 = PhotoMeta::load_sidecar(&photo)
            .expect("load disk1")
            .expect("sidecar exists");
        assert_eq!(disk1.revision, 1);
        assert_eq!(disk1.rating, 4);

        // Optimistic concurrency check succeeds on matching revision and advances to 2.
        meta.flag = CullFlag::Pick;
        meta.save_sidecar_checked_in_place(&photo, Some(1))
            .expect("checked save with expected=1");
        assert_eq!(meta.revision, 2);

        // Stale expected_revision fails with SidecarConflict and does NOT mutate in-memory revision.
        let conflict = meta.save_sidecar_checked_in_place(&photo, Some(1));
        assert!(matches!(
            conflict,
            Err(CoreError::SidecarConflict {
                expected: 1,
                actual: 2
            })
        ));
        assert_eq!(meta.revision, 2);

        // SessionCatalog::save_entry_sidecar_mut advances catalog entry revision in place.
        let mut catalog = SessionCatalog::from_paths(Some(dir.clone()), vec![photo.clone()]);
        assert_eq!(catalog.entries[0].meta.revision, 2);
        catalog.set_rating(0, 5);
        catalog
            .save_entry_sidecar_mut(0)
            .expect("save_entry_sidecar_mut")
            .expect("entry existed");
        assert_eq!(catalog.entries[0].meta.revision, 3);
        let disk3 = PhotoMeta::load_sidecar(&photo)
            .expect("load disk3")
            .expect("sidecar exists");
        assert_eq!(disk3.revision, 3);
        assert_eq!(disk3.rating, 5);

        let _ = std::fs::remove_file(&sc_path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn linear_to_srgb_lut_matches_exact_iec_transfer_and_compiled_bypass() {
        assert_eq!(linear_to_srgb(0.0), 0.0);
        assert_eq!(linear_to_srgb(-0.5), 0.0);
        assert_eq!(linear_to_srgb(f32::NAN), 0.0);
        assert_eq!(linear_to_srgb(1.0), 1.0);
        assert_eq!(linear_to_srgb(2.0), 1.0);

        let mut prev = -1.0_f32;
        let mut max_err = 0.0_f32;
        for i in 0..=2000 {
            let x = i as f32 / 2000.0;
            let fast = linear_to_srgb(x);
            let exact = if x <= 0.0031308 {
                12.92 * x
            } else {
                1.055 * x.powf(1.0 / 2.4) - 0.055
            };
            assert!(
                fast >= prev - 1e-7,
                "LUT must be monotonic at {x}: {fast} < {prev}"
            );
            prev = fast;
            max_err = max_err.max((fast - exact).abs());
        }
        assert!(
            max_err < 5e-5,
            "4096-entry linear_to_srgb LUT max error {max_err} exceeded 5e-5"
        );

        let neutral = DevelopRecipe::default().compile();
        assert!(!neutral.tones_active);
        assert!(!neutral.contrast_active);
        assert!(!neutral.sat_active);
        assert_eq!(
            neutral.evaluate_pixel_scene_linear([0.18, 0.36, 0.72]),
            [0.18, 0.36, 0.72]
        );

        let active = DevelopRecipe {
            shadows: 0.2,
            contrast: 0.15,
            vibrance: 0.1,
            ..DevelopRecipe::default()
        }
        .compile();
        assert!(active.tones_active);
        assert!(active.contrast_active);
        assert!(active.sat_active);
    }

    #[test]
    fn film_curve_modes_monotonic_and_distinct() {
        let shadow = [0.04, 0.04, 0.04];
        let mid = [0.18, 0.18, 0.18];
        let hdr = [1.8, 1.8, 1.8];

        // FilmStandard matches legacy filmic_tonemap bit-for-bit.
        for px in [shadow, mid, hdr] {
            assert_eq!(
                filmic_tonemap_with_curve(px, FilmCurve::FilmStandard),
                filmic_tonemap(px)
            );
        }

        // FilmExtraShadow lifts shadows cleanly while keeping 0 -> 0.
        assert_eq!(
            filmic_tonemap_with_curve([0.0, 0.0, 0.0], FilmCurve::FilmExtraShadow),
            [0.0, 0.0, 0.0]
        );
        let std_sh = filmic_tonemap_with_curve(shadow, FilmCurve::FilmStandard)[0];
        let ext_sh = filmic_tonemap_with_curve(shadow, FilmCurve::FilmExtraShadow)[0];
        let hc_sh = filmic_tonemap_with_curve(shadow, FilmCurve::FilmHighContrast)[0];
        assert!(ext_sh > std_sh * 1.25, "ExtraShadow should lift shadows");
        assert!(hc_sh < std_sh * 0.85, "HighContrast should deepen shadows");

        // LinearResponse preserves 0..=1 linearly and hard-clamps >1.
        assert_eq!(
            filmic_tonemap_with_curve([0.85, 0.5, 1.8], FilmCurve::LinearResponse),
            [0.85, 0.5, 1.0]
        );

        // Monotonicity & finiteness across all curves from 0..4.0.
        for curve in FilmCurve::ALL {
            let mut prev = -1.0_f32;
            for i in 0..=400 {
                let x = i as f32 * 0.01;
                let y = filmic_tonemap_with_curve([x, x, x], curve)[0];
                assert!(y.is_finite() && (0.0..=1.0).contains(&y));
                assert!(
                    y + 1e-6 >= prev,
                    "{curve:?} non-monotonic at {x}: {y} < {prev}"
                );
                prev = y;
            }
        }
    }

    #[test]
    fn wb_kelvin_roundtrip_and_presets() {
        assert_eq!(temperature_to_kelvin(-1.0), 2000);
        assert_eq!(temperature_to_kelvin(0.0), 5500);
        assert_eq!(temperature_to_kelvin(1.0), 14000);
        assert_eq!(temperature_to_kelvin(f32::NAN), 5500);
        assert!((kelvin_to_temperature(5500) - 0.0).abs() < 1e-6);
        assert!((kelvin_to_temperature(2000) + 1.0).abs() < 1e-5);
        assert!((kelvin_to_temperature(14000) - 1.0).abs() < 1e-5);

        for k in [2000u32, 2800, 3200, 4200, 5500, 6500, 8000, 11000, 14000] {
            let t = kelvin_to_temperature(k);
            let k2 = temperature_to_kelvin(t);
            assert!(
                k.abs_diff(k2) <= 1,
                "Kelvin roundtrip drift for {k}: t={t} -> {k2}"
            );
        }

        assert_eq!(wb_preset_values(WbPreset::AsShot), Some((0.0, 0.0)));
        assert_eq!(wb_preset_values(WbPreset::Custom), None);
        let (tung_t, _) = wb_preset_values(WbPreset::Tungsten).expect("tungsten");
        let (shade_t, _) = wb_preset_values(WbPreset::Shade).expect("shade");
        assert!(tung_t < 0.0 && shade_t > 0.0);

        let mut recipe = DevelopRecipe::default();
        recipe.apply_wb_preset(WbPreset::Fluorescent);
        assert_eq!(recipe.wb_preset, WbPreset::Fluorescent);
        assert!(recipe.tint > 0.15, "Fluorescent should add magenta tint");
    }

    #[test]
    fn rgb_combined_curve_bakes_into_lut_and_preserves_luma() {
        let curves = ToneCurves {
            rgb: vec![[0.0, 0.05], [0.5, 0.65], [1.0, 0.95]],
            red: vec![[0.0, 0.0], [0.5, 0.40], [1.0, 1.0]],
            luma: vec![[0.0, 0.0], [0.5, 0.52], [1.0, 1.0]],
            ..ToneCurves::default()
        };
        assert!(!curves.is_identity());
        let lut = curves.bake_curve_lut_f32();
        let mid = sample_curve_lut(&lut, 0.5);
        let expected_r = ToneCurves::evaluate_channel(
            &curves.rgb,
            ToneCurves::evaluate_channel(&curves.red, 0.5),
        );
        let expected_g = ToneCurves::evaluate_channel(&curves.rgb, 0.5);
        let expected_l = ToneCurves::evaluate_channel(&curves.luma, 0.5);
        assert!((mid[0] - expected_r).abs() < 2e-3);
        assert!((mid[1] - expected_g).abs() < 2e-3);
        assert!((mid[2] - expected_g).abs() < 2e-3);
        assert!((mid[3] - expected_l).abs() < 2e-3);
    }

    #[test]
    fn per_channel_levels_master_wheel_and_bw_split_tones() {
        // Per-channel Red levels lifts Red without altering Green or Blue.
        let r_lvl = DevelopRecipe {
            levels: LevelsParams {
                red: ChannelLevels {
                    in_mid: 0.45,
                    ..ChannelLevels::default()
                },
                ..LevelsParams::default()
            },
            ..DevelopRecipe::default()
        }
        .compile();
        let px = [0.18, 0.18, 0.18];
        let out = r_lvl.evaluate_pixel_scene_linear(px);
        assert!(out[0] > 0.25, "Red channel levels should lift R: {out:?}");
        assert!((out[1] - 0.18).abs() < 1e-6);
        assert!((out[2] - 0.18).abs() < 1e-6);

        // Master color balance wheel shifts shadows, midtones, and highlights uniformly.
        let master = DevelopRecipe {
            master_wheel: ColorWheel::new(0.5, 0.0, 0.25),
            ..DevelopRecipe::default()
        }
        .compile();
        for tone in [0.03_f32, 0.18, 0.65] {
            let m = master.evaluate_pixel_scene_linear([tone, tone, tone]);
            assert!(
                m[0] > m[2] * 1.15 && m[1] > tone * 1.05,
                "master wheel should warm and lift tone {tone}: {m:?}"
            );
        }

        // B&W Split Toning tints shadows cool blue (220 deg) and highlights warm amber (35 deg)
        // while preserving luminance and pure black.
        let split_bw = DevelopRecipe {
            bw: BlackAndWhiteParams {
                enabled: true,
                split_shadow_hue: 220.0,
                split_shadow_sat: 0.6,
                split_highlight_hue: 35.0,
                split_highlight_sat: 0.6,
                ..BlackAndWhiteParams::default()
            },
            ..DevelopRecipe::default()
        }
        .compile();
        assert_eq!(
            split_bw.evaluate_pixel_scene_linear([0.0, 0.0, 0.0]),
            [0.0, 0.0, 0.0]
        );
        let sh = split_bw.evaluate_pixel_scene_linear([0.04, 0.04, 0.04]);
        let hi = split_bw.evaluate_pixel_scene_linear([0.65, 0.65, 0.65]);
        assert!(sh[2] > sh[0], "B&W shadows should be cool blue: {sh:?}");
        assert!(hi[0] > hi[2], "B&W highlights should be warm amber: {hi:?}");
        let sh_y = 0.2126 * sh[0] + 0.7152 * sh[1] + 0.0722 * sh[2];
        assert!((sh_y - 0.04).abs() < 1e-4, "split toning preserves luma");
    }

    #[test]
    fn phase2_advanced_color_skin_tone_lens_and_keystone_math() {
        // 1. Advanced Color Editor uniformity pulls off-target hue toward target hue without touching other hues.
        let mut recipe = DevelopRecipe::default();
        let warm_orange = [0.45, 0.22, 0.05];
        let blue_sky = [0.04, 0.14, 0.55];
        let (h, s, v) = recipe
            .pick_color_slice_from_rgb(0, warm_orange)
            .expect("slice 0");
        assert!(h > 10.0 && h < 50.0 && s > 0.3 && v > 0.3);
        recipe.advanced_color[0].uniformity = 0.8;
        recipe.advanced_color[0].lightness = 0.5;
        let comp = recipe.compile();
        let orange_out = comp.evaluate_pixel_scene_linear(warm_orange);
        let blue_out = comp.evaluate_pixel_scene_linear(blue_sky);
        assert!(orange_out[0] > warm_orange[0] * 1.15);
        for ch in 0..3 {
            assert!((blue_out[ch] - blue_sky[ch]).abs() < 1e-5);
        }

        // 2. Skin Tone Uniformity pulls nearby reddish skin toward target skin hue without touching blue.
        let mut skin_rec = DevelopRecipe::default();
        skin_rec.pick_skin_tone_from_rgb([0.48, 0.28, 0.18]);
        skin_rec.skin_tone.uniformity_hue = 0.9;
        skin_rec.skin_tone.uniformity_sat = 0.7;
        let skin_comp = skin_rec.compile();
        let reddish_skin = [0.50, 0.22, 0.18];
        let (h_before, _, _) = sample_color_slice_from_rgb(reddish_skin);
        let (h_after, _, _) =
            sample_color_slice_from_rgb(skin_comp.evaluate_pixel_scene_linear(reddish_skin));
        assert!(
            (h_after - skin_rec.skin_tone.target_hue).abs()
                < (h_before - skin_rec.skin_tone.target_hue).abs(),
            "skin hue uniformity should move hue closer to target: {h_before} -> {h_after} (target {})",
            skin_rec.skin_tone.target_hue
        );
        let blue_skin = skin_comp.evaluate_pixel_scene_linear(blue_sky);
        for ch in 0..3 {
            assert!((blue_skin[ch] - blue_sky[ch]).abs() < 1e-5);
        }

        // 3. Keystone homography stays inside [0, 1]^2 across all corners & is identity when zero.
        let id_geo = GeometryParams::default();
        assert!(id_geo.is_keystone_identity());
        assert_eq!(id_geo.apply_keystone_uv(0.25, 0.75), [0.25, 0.75]);

        let ks_geo = GeometryParams {
            keystone_vertical: 0.8,
            keystone_horizontal: -0.6,
            keystone_aspect: 0.4,
            keystone_amount: 1.0,
            ..GeometryParams::default()
        };
        assert!(!ks_geo.is_identity());
        for ou in [0.0_f32, 0.5, 1.0] {
            for ov in [0.0_f32, 0.5, 1.0] {
                let [ku, kv] = ks_geo.apply_keystone_uv(ou, ov);
                assert!(
                    (0.0..=1.0).contains(&ku) && (0.0..=1.0).contains(&kv),
                    "keystone corner ({ou},{ov}) escaped [0,1]: ({ku},{kv})"
                );
            }
        }

        // 4. Radial lens distortion identity + symmetry.
        assert_eq!(
            apply_lens_distortion_uv([0.2, 0.8], 0.0, 1600, 1000),
            [0.2, 0.8]
        );
        let [du, dv] = apply_lens_distortion_uv([0.5, 0.5], 0.8, 1600, 1000);
        assert!((du - 0.5).abs() < 1e-6 && (dv - 0.5).abs() < 1e-6);
    }

    #[test]
    fn phase3_color_tags_virtual_variants_v2_compat_and_clipboard_autoselect() {
        // 1. ColorTag enum roundtrip & swatches.
        assert_eq!(ColorTag::ALL.len(), 8);
        for tag in ColorTag::ALL {
            assert_eq!(ColorTag::from_cli_str(tag.cli_slug()), Some(tag));
            assert!(!tag.label().is_empty());
            let rgb = tag.srgb_u8();
            assert!(rgb.iter().any(|&c| c > 0));
        }

        // 2. Faceted SessionCatalog filters (CullFilter + ColorTag + case-insensitive search_query).
        let paths = vec![
            PathBuf::from("/tmp/rune_p3_sunset.dng"),
            PathBuf::from("/tmp/rune_p3_portrait.raf"),
            PathBuf::from("/tmp/rune_p3_studio.cr3"),
        ];
        let mut catalog = SessionCatalog::from_paths(Some(PathBuf::from("/tmp")), paths);
        catalog.set_color_tag(0, ColorTag::Orange);
        catalog.set_color_tag(1, ColorTag::Green);
        catalog.set_color_tag(2, ColorTag::Orange);
        catalog.set_entry_exif_labels(
            0,
            Some("Leica Q3".to_owned()),
            Some("28mm f/1.7".to_owned()),
        );
        catalog.set_entry_exif_labels(
            1,
            Some("Fujifilm GFX100".to_owned()),
            Some("80mm f/1.7".to_owned()),
        );
        catalog.entries[2].meta.recipe.preset_name = Some("Editorial B&W".to_owned());

        catalog.set_color_tag_filter(Some(ColorTag::Orange));
        assert_eq!(catalog.filtered_indices(), vec![0, 2]);
        catalog.set_search_query("leica");
        assert_eq!(catalog.filtered_indices(), vec![0]);
        catalog.set_search_query("editorial");
        assert_eq!(catalog.filtered_indices(), vec![2]);
        catalog.set_color_tag_filter(None);
        catalog.set_search_query("80mm");
        assert_eq!(catalog.filtered_indices(), vec![1]);

        // 3. Virtual Variants lifecycle + v2 -> v3 sidecar backward compatibility.
        let v2_sidecar_json = r#"{
            "meta_version": 2,
            "revision": 3,
            "rating": 4,
            "flag": "Pick",
            "recipe": {
                "schema_version": 1,
                "exposure": 0.75,
                "contrast": 0.2,
                "highlights": 0.0,
                "shadows": 0.0,
                "whites": 0.0,
                "blacks": 0.0,
                "temperature": 0.0,
                "tint": 0.0,
                "vibrance": 0.0,
                "saturation": 0.0,
                "vignette": 0.0,
                "grain": 0.0,
                "preset_name": null
            }
        }"#;
        let mut meta = PhotoMeta::from_sidecar_json(v2_sidecar_json)
            .expect("v2 sidecar must load cleanly into v3 PhotoMeta");
        assert_eq!(meta.color_tag, ColorTag::None);
        assert_eq!(meta.variant_count(), 1);
        assert_eq!(meta.active_variant_index, 0);
        assert!((meta.recipe.exposure - 0.75).abs() < 1e-6);

        // Cannot delete the sole variant.
        assert!(!meta.delete_variant(0));
        assert_eq!(meta.variant_count(), 1);

        // Clone active variant -> inherits exposure 0.75 & rating 4.
        let v2_idx = meta.clone_variant(Some("Warm Print".to_owned()));
        assert_eq!(v2_idx, 1);
        assert_eq!(meta.variant_count(), 2);
        assert!((meta.recipe.exposure - 0.75).abs() < 1e-6);
        meta.recipe.temperature = 0.35;
        meta.color_tag = ColorTag::Orange;

        // New variant -> fresh Neutral recipe & clears tag/rating on new slot without losing v1/v2.
        let v3_idx = meta.new_variant(Some("B&W Study".to_owned()));
        assert_eq!(v3_idx, 2);
        assert_eq!(meta.variant_count(), 3);
        assert!(meta.recipe.is_neutral());
        meta.recipe.bw.enabled = true;

        // Select back to v2 and confirm edits were preserved.
        assert!(meta.select_variant(1));
        assert!((meta.recipe.temperature - 0.35).abs() < 1e-6);
        assert_eq!(meta.color_tag, ColorTag::Orange);

        // Disk roundtrip via save_sidecar_in_place syncs active edits to variants & writes v3.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let photo = std::env::temp_dir().join(format!(
            "rune_core_variants_test_{}_{nanos}.dng",
            std::process::id()
        ));
        meta.recipe.exposure = 1.1;
        let sc_path = meta.save_sidecar_in_place(&photo).expect("save v3 sidecar");
        let loaded = PhotoMeta::load_sidecar(&photo)
            .expect("load v3 sidecar")
            .expect("sidecar present");
        let _ = std::fs::remove_file(&sc_path);
        assert_eq!(loaded.meta_version, META_SCHEMA_VERSION);
        assert_eq!(loaded.variant_count(), 3);
        assert_eq!(loaded.active_variant_index, 1);
        assert!((loaded.recipe.exposure - 1.1).abs() < 1e-6);
        assert!((loaded.variants[1].recipe.exposure - 1.1).abs() < 1e-6);
        assert!(loaded.variants[2].recipe.bw.enabled);

        // Delete middle variant cleanly updates active index.
        let mut del_meta = loaded;
        assert!(del_meta.delete_variant(1));
        assert_eq!(del_meta.variant_count(), 2);
        assert_eq!(del_meta.active_variant_index, 1);
        assert!(del_meta.recipe.bw.enabled);

        // 4. Selective Adjustments Clipboard (ALL_CLIPBOARD_GROUPS + Autoselect Adjusted).
        assert_eq!(ALL_CLIPBOARD_GROUPS.len(), 14);
        assert!(modified_recipe_groups(&DevelopRecipe::default()).is_empty());

        let mut graded = DevelopRecipe {
            temperature: 0.2,
            exposure: 0.5,
            clarity: 0.25,
            vignette: -0.3,
            ..DevelopRecipe::default()
        };
        graded.lens.distortion = -0.2;
        graded.skin_tone.enabled = true;
        graded.geometry.straighten_deg = 2.0;
        let adjusted = modified_recipe_groups(&graded);
        assert_eq!(
            adjusted,
            vec![
                "wb",
                "tone",
                "skin_tone",
                "detail",
                "effects",
                "lens",
                "geometry"
            ]
        );

        // Every clipboard group copies cleanly via copy_recipe_groups.
        let all_ids: Vec<String> = ALL_CLIPBOARD_GROUPS
            .iter()
            .map(|(id, _)| (*id).to_owned())
            .collect();
        let mut dst = DevelopRecipe::default();
        copy_recipe_groups(&mut dst, &graded, &all_ids).expect("copy all 12 clipboard groups");
        assert_eq!(dst.temperature, graded.temperature);
        assert_eq!(dst.exposure, graded.exposure);
        assert_eq!(dst.skin_tone, graded.skin_tone);
        assert_eq!(dst.lens, graded.lens);
        assert_eq!(dst.geometry, graded.geometry);
    }

    #[test]
    fn phase4_layers_registry_patch_and_legacy_sidecar_compat() {
        let mut r = DevelopRecipe::default();
        assert!(r.is_neutral());

        let mut layer = AdjustmentLayer::new_with_mask(
            "l1",
            "Sky Gradient",
            MaskShape::LinearGradient {
                start: [0.5, 0.0],
                end: [0.5, 0.6],
                feather: 0.35,
            },
        );
        layer.adjustments.exposure = -0.8;
        layer.adjustments.clarity = 0.25;
        r.layers.push(layer);
        r.heal_spots.push(HealSpot {
            id: "h1".to_owned(),
            target: [0.3, 0.4],
            source: [0.4, 0.4],
            radius: 0.03,
            feather: 0.4,
            opacity: 1.0,
            mode: HealMode::Heal,
        });
        assert!(!r.is_neutral());
        assert!(modified_recipe_groups(&r).contains(&"layers"));

        // Sparse patch roundtrip preserves layers & heal_spots.
        let patch = recipe_to_sparse_patch(&r);
        let mut patched = DevelopRecipe::default();
        let diffs = patch
            .apply_to(&mut patched, None)
            .expect("apply layer patch");
        assert!(diffs.iter().any(|d| d.field == "layers"));
        assert!(diffs.iter().any(|d| d.field == "heal_spots"));
        assert_eq!(patched.layers, r.layers);
        assert_eq!(patched.heal_spots, r.heal_spots);

        // Group copy/reset symmetry for layers & heal.
        for grp in ["layers", "heal"] {
            let mut dst = DevelopRecipe::default();
            copy_recipe_groups(&mut dst, &r, &[grp.to_owned()]).unwrap();
            reset_recipe_group(&mut dst, grp).unwrap();
            assert!(
                dst.is_neutral(),
                "Phase 4 group '{grp}' failed copy/reset symmetry"
            );
        }
    }

    #[test]
    fn phase5_film_negative_session_folders_and_move_safety() {
        // 1. Film Negative optical inversion: orange C-41 mask neutralizes to clean black/gray, monotonic response.
        let p = FilmNegativeParams::default();
        let rebate = [p.base_r, p.base_g, p.base_b];
        let inv_rebate = invert_film_negative_linear(rebate, &p);
        assert_eq!(
            inv_rebate,
            [0.0, 0.0, 0.0],
            "clear film rebate must invert to pure black"
        );

        let mid_neg = [p.base_r * 0.25, p.base_g * 0.25, p.base_b * 0.25];
        let inv_mid = invert_film_negative_linear(mid_neg, &p);
        assert!(
            (inv_mid[0] - inv_mid[1]).abs() < 1e-5 && (inv_mid[1] - inv_mid[2]).abs() < 1e-5,
            "neutral negative under orange mask must invert to neutral gray: {inv_mid:?}"
        );
        let dense_neg = [p.base_r * 0.04, p.base_g * 0.04, p.base_b * 0.04];
        let inv_hi = invert_film_negative_linear(dense_neg, &p);
        assert!(
            inv_hi[0] > inv_mid[0] && inv_mid[0] > inv_rebate[0],
            "denser negative must produce brighter positive: hi={inv_hi:?} mid={inv_mid:?}"
        );

        // Adversarial zero/NaN base safety (never divides by zero or produces NaN).
        let bad_p = FilmNegativeParams {
            base_r: 0.0,
            base_g: f32::NAN,
            base_b: -1.0,
            density: 0.2,
            contrast: 0.15,
            black_clip: 0.4,
            white_clip: 0.3,
        };
        let safe_out = invert_film_negative_linear([0.5, 0.3, 0.2], &bad_p);
        assert!(safe_out.iter().all(|v| v.is_finite() && *v >= 0.0));

        // Base picker + dot-path + patch + copy/reset symmetry.
        let mut r = DevelopRecipe::default();
        let picked = r.convert_film_negative_from_base([0.88, 0.52, 0.31]);
        assert_eq!(r.processing_mode, ProcessingMode::FilmNegative);
        assert!((picked[0] - 0.88).abs() < 1e-6);
        set_param_by_str(&mut r, "film_negative.density", "0.35", false, None).unwrap();
        set_param_by_str(&mut r, "film_negative.contrast", "0.2", false, None).unwrap();
        assert!((get_scalar_by_path(&r, "film_negative.density").unwrap() - 0.35).abs() < 1e-6);
        assert!(modified_recipe_groups(&r).contains(&"film_negative"));

        let patch = recipe_to_sparse_patch(&r);
        let mut patched = DevelopRecipe::default();
        patch.apply_to(&mut patched, None).unwrap();
        assert_eq!(patched.processing_mode, ProcessingMode::FilmNegative);
        assert_eq!(patched.film_negative, r.film_negative);

        let mut dst = DevelopRecipe::default();
        copy_recipe_groups(&mut dst, &r, &["film_negative".to_owned()]).unwrap();
        assert_eq!(dst.processing_mode, ProcessingMode::FilmNegative);
        reset_recipe_group(&mut dst, "film_negative").unwrap();
        assert!(dst.is_neutral());

        // 2. 4-Folder Capture One Session + move_photo_with_sidecar non-clobber safety.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "rune_core_session_p5_{}_{nanos}",
            std::process::id()
        ));
        let layout = create_capture_one_session(&root).expect("create 4-folder session");
        assert!(layout.capture.is_dir());
        assert!(layout.selects.is_dir());
        assert!(layout.output.is_dir());
        assert!(layout.trash.is_dir());

        let photo = layout.capture.join("roll01_001.dng");
        std::fs::write(&photo, b"raw-bytes").unwrap();
        r.save_sidecar(&photo).unwrap();
        let src_sc = DevelopRecipe::sidecar_path_for(&photo);
        assert!(src_sc.is_file());

        let moved = move_photo_with_sidecar(&photo, &layout.selects).expect("move to Selects");
        assert_eq!(moved, layout.selects.join("roll01_001.dng"));
        assert!(moved.is_file());
        assert!(!photo.exists());
        assert!(!src_sc.exists());
        assert!(DevelopRecipe::sidecar_path_for(&moved).is_file());

        // Non-clobber safety: moving another file with the same name into Selects must FAIL.
        let dup = layout.capture.join("roll01_001.dng");
        std::fs::write(&dup, b"new-bytes").unwrap();
        assert!(move_photo_with_sidecar(&dup, &layout.selects).is_err());
        assert_eq!(std::fs::read(&moved).unwrap(), b"raw-bytes");

        let _ = std::fs::remove_dir_all(&root);
    }
}
