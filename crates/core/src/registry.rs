//! Single-source Parameter Registry for GUI ↔ CLI parity by construction.
//!
//! Every scalar and structured develop parameter lives here with its canonical
//! dot-path ID, display label, tool group, bounds, default value, and accessors.
//! Both `crates/app` (GUI), `crates/cli` (`rd edit --set`, `rd schema`,
//! `rd cli search`), `DevelopRecipe::sanitized()`, and `FieldDiff` consume this
//! registry so new darkroom features are automatically exposed to the CLI.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{
    ADVANCED_COLOR_SLICE_COUNT, ClarityMethod, CoreError, DevelopRecipe, FilmCurve, GrainType,
    HSL_BAND_COUNT, HSL_BAND_NAMES, ProcessingMode, VignetteMethod, WbPreset, builtin_presets,
    geometry::{CompositionGrid, CropAspect},
    patch::FieldDiff,
};

/// Logical tool group matching both GUI tabs and CLI `--only` / `--reset` scopes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamGroup {
    Tone,
    WhiteBalance,
    Color,
    Wheels,
    Hsl,
    SkinTone,
    Levels,
    Curves,
    Detail,
    Bw,
    Effects,
    Lens,
    Geometry,
    Layers,
    FilmNegative,
}

impl ParamGroup {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tone => "tone",
            Self::WhiteBalance => "wb",
            Self::Color => "color",
            Self::Wheels => "wheels",
            Self::Hsl => "hsl",
            Self::SkinTone => "skin_tone",
            Self::Levels => "levels",
            Self::Curves => "curves",
            Self::Detail => "detail",
            Self::Bw => "bw",
            Self::Effects => "effects",
            Self::Lens => "lens",
            Self::Geometry => "geometry",
            Self::Layers => "layers",
            Self::FilmNegative => "film_negative",
        }
    }
}

/// Declarative metadata + accessors for a top-level scalar `DevelopRecipe` slider.
pub struct ScalarParamSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub group: ParamGroup,
    pub description: &'static str,
    pub min: f32,
    pub max: f32,
    pub recommended_min: f32,
    pub recommended_max: f32,
    pub default: f32,
    pub step: f32,
    pub unit: &'static str,
    pub get: fn(&DevelopRecipe) -> f32,
    pub set: fn(&mut DevelopRecipe, f32),
}

/// Canonical metadata for all 23 top-level scalar sliders in `DevelopRecipe`.
pub const SCALAR_PARAMS: &[ScalarParamSpec] = &[
    ScalarParamSpec {
        id: "exposure",
        label: "Exposure",
        group: ParamGroup::Tone,
        description: "Exposure compensation in photographic stops (EV).",
        min: -5.0,
        max: 5.0,
        recommended_min: -3.0,
        recommended_max: 3.0,
        default: 0.0,
        step: 0.05,
        unit: "EV",
        get: |r| r.exposure,
        set: |r, v| r.exposure = v,
    },
    ScalarParamSpec {
        id: "contrast",
        label: "Contrast",
        group: ParamGroup::Tone,
        description: "Filmic S-curve contrast around 18% middle gray.",
        min: -1.0,
        max: 1.0,
        recommended_min: -0.6,
        recommended_max: 0.6,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.contrast,
        set: |r, v| r.contrast = v,
    },
    ScalarParamSpec {
        id: "brightness",
        label: "Brightness",
        group: ParamGroup::Tone,
        description: "Midtone-weighted luminance lift preserving black/white endpoints.",
        min: -1.0,
        max: 1.0,
        recommended_min: -0.5,
        recommended_max: 0.5,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.brightness,
        set: |r, v| r.brightness = v,
    },
    ScalarParamSpec {
        id: "highlights",
        label: "Highlights",
        group: ParamGroup::Tone,
        description: "Highlight recovery (negative) or boost (positive).",
        min: -1.0,
        max: 1.0,
        recommended_min: -0.8,
        recommended_max: 0.5,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.highlights,
        set: |r, v| r.highlights = v,
    },
    ScalarParamSpec {
        id: "shadows",
        label: "Shadows",
        group: ParamGroup::Tone,
        description: "Shadow lift (positive) or deepen (negative).",
        min: -1.0,
        max: 1.0,
        recommended_min: -0.5,
        recommended_max: 0.7,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.shadows,
        set: |r, v| r.shadows = v,
    },
    ScalarParamSpec {
        id: "whites",
        label: "Whites",
        group: ParamGroup::Tone,
        description: "Upper-end white point trim.",
        min: -1.0,
        max: 1.0,
        recommended_min: -0.5,
        recommended_max: 0.5,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.whites,
        set: |r, v| r.whites = v,
    },
    ScalarParamSpec {
        id: "blacks",
        label: "Blacks",
        group: ParamGroup::Tone,
        description: "Lower-end black point trim.",
        min: -1.0,
        max: 1.0,
        recommended_min: -0.5,
        recommended_max: 0.5,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.blacks,
        set: |r, v| r.blacks = v,
    },
    ScalarParamSpec {
        id: "temperature",
        label: "Temperature",
        group: ParamGroup::WhiteBalance,
        description: "White balance blue (-1) to amber (+1) shift.",
        min: -1.0,
        max: 1.0,
        recommended_min: -0.5,
        recommended_max: 0.5,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.temperature,
        set: |r, v| r.temperature = v,
    },
    ScalarParamSpec {
        id: "tint",
        label: "Tint",
        group: ParamGroup::WhiteBalance,
        description: "White balance green (-1) to magenta (+1) shift.",
        min: -1.0,
        max: 1.0,
        recommended_min: -0.4,
        recommended_max: 0.4,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.tint,
        set: |r, v| r.tint = v,
    },
    ScalarParamSpec {
        id: "vibrance",
        label: "Vibrance",
        group: ParamGroup::Color,
        description: "Perceptual chroma boost protecting skin tones and saturated colors.",
        min: -1.0,
        max: 1.0,
        recommended_min: -0.4,
        recommended_max: 0.5,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.vibrance,
        set: |r, v| r.vibrance = v,
    },
    ScalarParamSpec {
        id: "saturation",
        label: "Saturation",
        group: ParamGroup::Color,
        description: "Global color saturation (-1 monochrome .. +1 vivid).",
        min: -1.0,
        max: 1.0,
        recommended_min: -1.0,
        recommended_max: 0.4,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.saturation,
        set: |r, v| r.saturation = v,
    },
    ScalarParamSpec {
        id: "clarity",
        label: "Clarity",
        group: ParamGroup::Detail,
        description: "Local midtone contrast.",
        min: -1.0,
        max: 1.0,
        recommended_min: -0.3,
        recommended_max: 0.4,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.clarity,
        set: |r, v| r.clarity = v,
    },
    ScalarParamSpec {
        id: "structure",
        label: "Structure",
        group: ParamGroup::Detail,
        description: "Fine micro-contrast texture enhancement.",
        min: -1.0,
        max: 1.0,
        recommended_min: -0.3,
        recommended_max: 0.4,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.structure,
        set: |r, v| r.structure = v,
    },
    ScalarParamSpec {
        id: "dehaze",
        label: "Dehaze",
        group: ParamGroup::Detail,
        description: "Atmospheric contrast / airlight removal.",
        min: -1.0,
        max: 1.0,
        recommended_min: -0.3,
        recommended_max: 0.4,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.dehaze,
        set: |r, v| r.dehaze = v,
    },
    ScalarParamSpec {
        id: "sharpen",
        label: "Sharpen",
        group: ParamGroup::Detail,
        description: "Unsharp mask sharpening amount (0..1).",
        min: 0.0,
        max: 1.0,
        recommended_min: 0.0,
        recommended_max: 0.5,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.sharpen,
        set: |r, v| r.sharpen = v,
    },
    ScalarParamSpec {
        id: "sharpen_radius",
        label: "Sharpen Radius",
        group: ParamGroup::Detail,
        description: "Unsharp mask radius in pixels (0.5..4.0).",
        min: 0.5,
        max: 4.0,
        recommended_min: 0.5,
        recommended_max: 2.5,
        default: 1.0,
        step: 0.1,
        unit: "px",
        get: |r| r.sharpen_radius,
        set: |r, v| r.sharpen_radius = v,
    },
    ScalarParamSpec {
        id: "sharpen_threshold",
        label: "Sharpen Threshold",
        group: ParamGroup::Detail,
        description: "Minimum edge contrast required before sharpening applies (0..1).",
        min: 0.0,
        max: 1.0,
        recommended_min: 0.0,
        recommended_max: 0.5,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.sharpen_threshold,
        set: |r, v| r.sharpen_threshold = v,
    },
    ScalarParamSpec {
        id: "sharpen_halo",
        label: "Halo Suppression",
        group: ParamGroup::Detail,
        description: "Suppresses bright/dark ringing halos along high-contrast edges (0..1).",
        min: 0.0,
        max: 1.0,
        recommended_min: 0.0,
        recommended_max: 0.8,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.sharpen_halo,
        set: |r, v| r.sharpen_halo = v,
    },
    ScalarParamSpec {
        id: "noise_reduction",
        label: "Noise Reduction",
        group: ParamGroup::Detail,
        description: "Edge-aware luminance and combined noise reduction (0..1).",
        min: 0.0,
        max: 1.0,
        recommended_min: 0.0,
        recommended_max: 0.6,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.noise_reduction,
        set: |r, v| r.noise_reduction = v,
    },
    ScalarParamSpec {
        id: "nr_color",
        label: "Color Noise Reduction",
        group: ParamGroup::Detail,
        description: "Edge-aware chroma/color noise reduction preserving luminance detail (0..1).",
        min: 0.0,
        max: 1.0,
        recommended_min: 0.0,
        recommended_max: 0.8,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.nr_color,
        set: |r, v| r.nr_color = v,
    },
    ScalarParamSpec {
        id: "vignette",
        label: "Vignette",
        group: ParamGroup::Effects,
        description: "Crop-aware optical radial vignette (-1 darken .. +1 brighten).",
        min: -1.0,
        max: 1.0,
        recommended_min: -0.5,
        recommended_max: 0.3,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.vignette,
        set: |r, v| r.vignette = v,
    },
    ScalarParamSpec {
        id: "grain",
        label: "Film Grain",
        group: ParamGroup::Effects,
        description: "Subtle midtone-weighted photographic film grain impact (0..1).",
        min: 0.0,
        max: 1.0,
        recommended_min: 0.0,
        recommended_max: 0.4,
        default: 0.0,
        step: 0.02,
        unit: "norm",
        get: |r| r.grain,
        set: |r, v| r.grain = v,
    },
    ScalarParamSpec {
        id: "grain_size",
        label: "Grain Granularity",
        group: ParamGroup::Effects,
        description: "Photographic film grain clump size / granularity (0..1, default 0.35).",
        min: 0.0,
        max: 1.0,
        recommended_min: 0.1,
        recommended_max: 0.7,
        default: 0.35,
        step: 0.02,
        unit: "norm",
        get: |r| r.grain_size,
        set: |r, v| r.grain_size = v,
    },
];

/// Serializable descriptor returned by `rd schema` and `rd cli search`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamDescriptor {
    pub id: String,
    pub label: String,
    pub group: String,
    pub kind: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,
}

/// Resolves canonical aliases and normalizes redundant group prefixes
/// (e.g. `temp` -> `temperature`, `levels.levels.in_black` -> `levels.in_black`,
/// `wheels.shadows_wheel.hue_x` / `wheels.shadows.hue_x` -> `shadows_wheel.hue_x`).
pub fn canonical_param_key(raw: &str) -> String {
    let mut lower = raw.trim().to_ascii_lowercase();
    // Collapse redundant group prefixes emitted when a client joins `group` + `id`.
    for prefix in [
        "levels.levels.",
        "curves.curves.",
        "bw.bw.",
        "hsl.hsl.",
        "lens.lens.",
        "skin_tone.skin_tone.",
        "advanced_color.advanced_color.",
        "geometry.geometry.",
    ] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let single = prefix.split('.').next().unwrap_or("");
            lower = format!("{single}.{rest}");
            break;
        }
    }
    if let Some(rest) = lower.strip_prefix("wheels.") {
        lower = rest.to_owned();
    }
    for (short, canonical) in [
        ("shadows.", "shadows_wheel."),
        ("midtones.", "midtones_wheel."),
        ("highlights.", "highlights_wheel."),
        ("master.", "master_wheel."),
    ] {
        if let Some(rest) = lower.strip_prefix(short) {
            lower = format!("{canonical}{rest}");
            break;
        }
    }
    match lower.as_str() {
        "temp" | "wb_temp" => "temperature".to_owned(),
        "wb_tint" => "tint".to_owned(),
        "wb" | "white_balance_preset" => "wb_preset".to_owned(),
        "curve" | "base_curve" => "film_curve".to_owned(),
        "exp" | "ev" => "exposure".to_owned(),
        "sat" => "saturation".to_owned(),
        "vib" => "vibrance".to_owned(),
        "denoise" | "nr" => "noise_reduction".to_owned(),
        "color_nr" | "chroma_nr" => "nr_color".to_owned(),
        "halo" | "halo_suppression" => "sharpen_halo".to_owned(),
        "granularity" | "grain_granularity" => "grain_size".to_owned(),
        "vignette_style" => "vignette_method".to_owned(),
        "ca" | "chromatic_aberration" => "lens.chromatic_aberration".to_owned(),
        "distortion" => "lens.distortion".to_owned(),
        "purple_fringing" => "lens.purple_fringing".to_owned(),
        "light_falloff" => "lens.light_falloff".to_owned(),
        "keystone_vertical" => "geometry.keystone_vertical".to_owned(),
        "keystone_horizontal" => "geometry.keystone_horizontal".to_owned(),
        "keystone_aspect" => "geometry.keystone_aspect".to_owned(),
        "keystone_amount" => "geometry.keystone_amount".to_owned(),
        "straighten" => "geometry.straighten_deg".to_owned(),
        "rotate" => "geometry.rotate_90".to_owned(),
        "aspect" => "geometry.aspect_preset".to_owned(),
        _ => lower,
    }
}

/// Looks up a top-level scalar parameter spec by canonical ID or alias.
pub fn find_scalar_param(key: &str) -> Option<&'static ScalarParamSpec> {
    let canonical = canonical_param_key(key);
    SCALAR_PARAMS.iter().find(|spec| spec.id == canonical)
}

/// Maps an HSL band name (`red..magenta`) or index (`0..7`) to its array index.
pub fn parse_hsl_band_index(name: &str) -> Option<usize> {
    let lower = name.trim().to_ascii_lowercase();
    if let Ok(idx) = lower.parse::<usize>()
        && idx < HSL_BAND_COUNT
    {
        return Some(idx);
    }
    HSL_BAND_NAMES
        .iter()
        .position(|n| n.eq_ignore_ascii_case(&lower))
}

/// Returns every parameter registered across `DevelopRecipe` (scalars + wheels + HSL +
/// levels + B&W + geometry + curves) for `rd schema` and `rd cli search`.
pub fn all_param_descriptors() -> Vec<ParamDescriptor> {
    let mut out = Vec::with_capacity(64);
    for s in SCALAR_PARAMS {
        out.push(ParamDescriptor {
            id: s.id.to_owned(),
            label: s.label.to_owned(),
            group: s.group.as_str().to_owned(),
            kind: "f32".to_owned(),
            description: s.description.to_owned(),
            min: Some(s.min),
            max: Some(s.max),
            default: Some(json!(s.default)),
            unit: Some(s.unit.to_owned()),
            enum_values: None,
        });
    }

    out.push(ParamDescriptor {
        id: "film_curve".to_owned(),
        label: "Film Curve".to_owned(),
        group: ParamGroup::Tone.as_str().to_owned(),
        kind: "enum".to_owned(),
        description: "Base Characteristics tone response film curve.".to_owned(),
        min: None,
        max: None,
        default: Some(json!("film_standard")),
        unit: None,
        enum_values: Some(
            FilmCurve::ALL
                .iter()
                .map(|c| c.cli_slug().to_owned())
                .collect(),
        ),
    });
    out.push(ParamDescriptor {
        id: "wb_preset".to_owned(),
        label: "White Balance Preset".to_owned(),
        group: ParamGroup::WhiteBalance.as_str().to_owned(),
        kind: "enum".to_owned(),
        description: "Standard illuminant white balance preset.".to_owned(),
        min: None,
        max: None,
        default: Some(json!("as_shot")),
        unit: None,
        enum_values: Some(
            WbPreset::ALL
                .iter()
                .map(|p| p.cli_slug().to_owned())
                .collect(),
        ),
    });
    out.push(ParamDescriptor {
        id: "clarity_method".to_owned(),
        label: "Clarity Method".to_owned(),
        group: ParamGroup::Detail.as_str().to_owned(),
        kind: "enum".to_owned(),
        description: "Local contrast Clarity algorithm method.".to_owned(),
        min: None,
        max: None,
        default: Some(json!("natural")),
        unit: None,
        enum_values: Some(
            ClarityMethod::ALL
                .iter()
                .map(|m| m.cli_slug().to_owned())
                .collect(),
        ),
    });
    out.push(ParamDescriptor {
        id: "grain_type".to_owned(),
        label: "Film Grain Type".to_owned(),
        group: ParamGroup::Effects.as_str().to_owned(),
        kind: "enum".to_owned(),
        description: "Photographic film grain emulsion profile.".to_owned(),
        min: None,
        max: None,
        default: Some(json!("fine")),
        unit: None,
        enum_values: Some(
            GrainType::ALL
                .iter()
                .map(|g| g.cli_slug().to_owned())
                .collect(),
        ),
    });
    out.push(ParamDescriptor {
        id: "vignette_method".to_owned(),
        label: "Vignette Method".to_owned(),
        group: ParamGroup::Effects.as_str().to_owned(),
        kind: "enum".to_owned(),
        description: "Crop-aware optical vignette falloff geometry.".to_owned(),
        min: None,
        max: None,
        default: Some(json!("elliptic_on_crop")),
        unit: None,
        enum_values: Some(
            VignetteMethod::ALL
                .iter()
                .map(|m| m.cli_slug().to_owned())
                .collect(),
        ),
    });

    for (wheel_id, wheel_label) in [
        ("shadows_wheel", "Shadows Wheel"),
        ("midtones_wheel", "Midtones Wheel"),
        ("highlights_wheel", "Highlights Wheel"),
        ("master_wheel", "Master Wheel"),
    ] {
        for (comp, desc) in [
            (
                "hue_x",
                "Chroma X coordinate inside unit color disk (-1..1)",
            ),
            (
                "hue_y",
                "Chroma Y coordinate inside unit color disk (-1..1)",
            ),
            ("luma", "Wheel tonal lightness adjustment (-1..1)"),
        ] {
            out.push(ParamDescriptor {
                id: format!("{wheel_id}.{comp}"),
                label: format!("{wheel_label} {comp}"),
                group: ParamGroup::Wheels.as_str().to_owned(),
                kind: "f32".to_owned(),
                description: desc.to_owned(),
                min: Some(-1.0),
                max: Some(1.0),
                default: Some(json!(0.0)),
                unit: Some("norm".to_owned()),
                enum_values: None,
            });
        }
    }

    for band in HSL_BAND_NAMES {
        let b_low = band.to_ascii_lowercase();
        for (comp, desc) in [
            ("hue", "Hue shift for this color band (-1..1, ±30 deg)"),
            ("saturation", "Saturation shift for this color band (-1..1)"),
            ("luminance", "Luminance shift for this color band (-1..1)"),
        ] {
            out.push(ParamDescriptor {
                id: format!("hsl.{b_low}.{comp}"),
                label: format!("HSL {band} {comp}"),
                group: ParamGroup::Hsl.as_str().to_owned(),
                kind: "f32".to_owned(),
                description: desc.to_owned(),
                min: Some(-1.0),
                max: Some(1.0),
                default: Some(json!(0.0)),
                unit: Some("norm".to_owned()),
                enum_values: None,
            });
        }
    }

    for (prefix, label_prefix) in [
        ("levels", "Levels"),
        ("levels.red", "Levels Red"),
        ("levels.green", "Levels Green"),
        ("levels.blue", "Levels Blue"),
    ] {
        for (field, label, min, max, def, desc) in [
            (
                "in_black",
                "In Black",
                0.0,
                0.95,
                0.0,
                "Input black clipping point.",
            ),
            (
                "in_mid",
                "In Midtones",
                -1.0,
                1.0,
                0.0,
                "Input midtone gamma offset.",
            ),
            (
                "in_white",
                "In White",
                0.05,
                1.0,
                1.0,
                "Input white clipping point.",
            ),
            (
                "out_black",
                "Out Black",
                0.0,
                1.0,
                0.0,
                "Output shadow floor.",
            ),
            (
                "out_white",
                "Out White",
                0.0,
                1.0,
                1.0,
                "Output highlight ceiling.",
            ),
        ] {
            out.push(ParamDescriptor {
                id: format!("{prefix}.{field}"),
                label: format!("{label_prefix} {label}"),
                group: ParamGroup::Levels.as_str().to_owned(),
                kind: "f32".to_owned(),
                description: desc.to_owned(),
                min: Some(min),
                max: Some(max),
                default: Some(json!(def)),
                unit: Some("norm".to_owned()),
                enum_values: None,
            });
        }
    }

    out.push(ParamDescriptor {
        id: "bw.enabled".to_owned(),
        label: "Black & White Enabled".to_owned(),
        group: ParamGroup::Bw.as_str().to_owned(),
        kind: "bool".to_owned(),
        description: "Enable 6-channel Black & White mixer and Split Toning.".to_owned(),
        min: None,
        max: None,
        default: Some(json!(false)),
        unit: None,
        enum_values: None,
    });
    for ch in ["red", "yellow", "green", "cyan", "blue", "magenta"] {
        out.push(ParamDescriptor {
            id: format!("bw.{ch}"),
            label: format!("B&W {ch}"),
            group: ParamGroup::Bw.as_str().to_owned(),
            kind: "f32".to_owned(),
            description: format!("Black & White luminance weight for {ch} hues (-1..1)."),
            min: Some(-1.0),
            max: Some(1.0),
            default: Some(json!(0.0)),
            unit: Some("norm".to_owned()),
            enum_values: None,
        });
    }
    for (id, label, max, unit, desc) in [
        (
            "bw.split_shadow_hue",
            "B&W Split Shadow Hue",
            360.0,
            "deg",
            "B&W split toning shadow hue angle in degrees (0..360).",
        ),
        (
            "bw.split_shadow_sat",
            "B&W Split Shadow Saturation",
            1.0,
            "norm",
            "B&W split toning shadow saturation (0..1).",
        ),
        (
            "bw.split_highlight_hue",
            "B&W Split Highlight Hue",
            360.0,
            "deg",
            "B&W split toning highlight hue angle in degrees (0..360).",
        ),
        (
            "bw.split_highlight_sat",
            "B&W Split Highlight Saturation",
            1.0,
            "norm",
            "B&W split toning highlight saturation (0..1).",
        ),
    ] {
        out.push(ParamDescriptor {
            id: id.to_owned(),
            label: label.to_owned(),
            group: ParamGroup::Bw.as_str().to_owned(),
            kind: "f32".to_owned(),
            description: desc.to_owned(),
            min: Some(0.0),
            max: Some(max),
            default: Some(json!(0.0)),
            unit: Some(unit.to_owned()),
            enum_values: None,
        });
    }

    for ch in ["rgb", "luma", "red", "green", "blue"] {
        out.push(ParamDescriptor {
            id: format!("curves.{ch}"),
            label: format!("Tone Curve {ch}"),
            group: ParamGroup::Curves.as_str().to_owned(),
            kind: "points".to_owned(),
            description: format!(
                "Monotonic cubic Hermite control points for {ch} curve ([[x,y],...])."
            ),
            min: Some(0.0),
            max: Some(1.0),
            default: Some(json!([[0.0, 0.0], [1.0, 1.0]])),
            unit: None,
            enum_values: None,
        });
    }

    out.push(ParamDescriptor {
        id: "geometry.crop".to_owned(),
        label: "Crop Rect".to_owned(),
        group: ParamGroup::Geometry.as_str().to_owned(),
        kind: "[f32; 4]".to_owned(),
        description: "Normalized crop rectangle [x, y, w, h] in oriented-frame [0..1] space."
            .to_owned(),
        min: Some(0.0),
        max: Some(1.0),
        default: Some(json!([0.0, 0.0, 1.0, 1.0])),
        unit: Some("norm".to_owned()),
        enum_values: None,
    });
    out.push(ParamDescriptor {
        id: "geometry.straighten_deg".to_owned(),
        label: "Straighten".to_owned(),
        group: ParamGroup::Geometry.as_str().to_owned(),
        kind: "f32".to_owned(),
        description: "Fine horizon straighten rotation in degrees (-45..45).".to_owned(),
        min: Some(-45.0),
        max: Some(45.0),
        default: Some(json!(0.0)),
        unit: Some("deg".to_owned()),
        enum_values: None,
    });
    out.push(ParamDescriptor {
        id: "geometry.rotate_90".to_owned(),
        label: "Rotate 90".to_owned(),
        group: ParamGroup::Geometry.as_str().to_owned(),
        kind: "i8".to_owned(),
        description: "Clockwise quarter turns (0, 1, 2, 3).".to_owned(),
        min: Some(0.0),
        max: Some(3.0),
        default: Some(json!(0)),
        unit: Some("turns".to_owned()),
        enum_values: None,
    });
    out.push(ParamDescriptor {
        id: "geometry.flip_h".to_owned(),
        label: "Flip Horizontal".to_owned(),
        group: ParamGroup::Geometry.as_str().to_owned(),
        kind: "bool".to_owned(),
        description: "Horizontal mirror flip.".to_owned(),
        min: None,
        max: None,
        default: Some(json!(false)),
        unit: None,
        enum_values: None,
    });
    out.push(ParamDescriptor {
        id: "geometry.flip_v".to_owned(),
        label: "Flip Vertical".to_owned(),
        group: ParamGroup::Geometry.as_str().to_owned(),
        kind: "bool".to_owned(),
        description: "Vertical mirror flip.".to_owned(),
        min: None,
        max: None,
        default: Some(json!(false)),
        unit: None,
        enum_values: None,
    });
    out.push(ParamDescriptor {
        id: "geometry.aspect_preset".to_owned(),
        label: "Aspect Preset".to_owned(),
        group: ParamGroup::Geometry.as_str().to_owned(),
        kind: "enum".to_owned(),
        description: "Crop aspect ratio preset.".to_owned(),
        min: None,
        max: None,
        default: Some(json!("free")),
        unit: None,
        enum_values: Some(
            CropAspect::ALL
                .iter()
                .map(|a| a.cli_slug().to_owned())
                .collect(),
        ),
    });
    out.push(ParamDescriptor {
        id: "geometry.grid".to_owned(),
        label: "Composition Grid".to_owned(),
        group: ParamGroup::Geometry.as_str().to_owned(),
        kind: "enum".to_owned(),
        description: "Crop composition overlay guide.".to_owned(),
        min: None,
        max: None,
        default: Some(json!("thirds")),
        unit: None,
        enum_values: Some(
            CompositionGrid::ALL
                .iter()
                .map(|g| g.cli_slug().to_owned())
                .collect(),
        ),
    });
    for (id, label, min, max, def, desc) in [
        (
            "geometry.keystone_vertical",
            "Keystone Vertical",
            -1.0,
            1.0,
            0.0,
            "Vertical perspective keystone correction (-1..1).",
        ),
        (
            "geometry.keystone_horizontal",
            "Keystone Horizontal",
            -1.0,
            1.0,
            0.0,
            "Horizontal perspective keystone correction (-1..1).",
        ),
        (
            "geometry.keystone_aspect",
            "Keystone Aspect",
            -0.5,
            1.0,
            0.0,
            "Keystone vertical aspect ratio compensation (-0.5..1.0).",
        ),
        (
            "geometry.keystone_amount",
            "Keystone Amount",
            0.0,
            1.0,
            0.8,
            "Keystone perspective correction strength multiplier (0..1, default 0.8).",
        ),
    ] {
        out.push(ParamDescriptor {
            id: id.to_owned(),
            label: label.to_owned(),
            group: ParamGroup::Geometry.as_str().to_owned(),
            kind: "f32".to_owned(),
            description: desc.to_owned(),
            min: Some(min),
            max: Some(max),
            default: Some(json!(def)),
            unit: Some("norm".to_owned()),
            enum_values: None,
        });
    }

    for idx in 0..ADVANCED_COLOR_SLICE_COUNT {
        let num = idx + 1;
        out.push(ParamDescriptor {
            id: format!("advanced_color.{idx}.enabled"),
            label: format!("Color Slice {num} Enabled"),
            group: ParamGroup::Color.as_str().to_owned(),
            kind: "bool".to_owned(),
            description: format!("Enable Advanced Color Editor slice {num}."),
            min: None,
            max: None,
            default: Some(json!(false)),
            unit: None,
            enum_values: None,
        });
        for (field, label, min, max, def, unit, desc) in [
            (
                "target_hue",
                "Target Hue",
                0.0,
                360.0,
                0.0,
                "deg",
                "Slice center hue angle (0..360 deg).",
            ),
            (
                "target_sat",
                "Target Sat",
                0.0,
                1.0,
                0.5,
                "norm",
                "Slice center saturation (0..1).",
            ),
            (
                "hue_range",
                "Hue Range",
                5.0,
                180.0,
                30.0,
                "deg",
                "Slice hue half-width (5..180 deg).",
            ),
            (
                "sat_range",
                "Sat Range",
                0.05,
                1.0,
                0.45,
                "norm",
                "Slice saturation half-width (0.05..1).",
            ),
            (
                "smoothness",
                "Smoothness",
                0.0,
                1.0,
                0.4,
                "norm",
                "Selection edge transition feather (0..1).",
            ),
            (
                "hue_shift",
                "Hue Shift",
                -1.0,
                1.0,
                0.0,
                "norm",
                "Hue rotation shift (-1..1, ±60 deg).",
            ),
            (
                "saturation",
                "Saturation",
                -1.0,
                1.0,
                0.0,
                "norm",
                "Saturation adjustment (-1..1).",
            ),
            (
                "lightness",
                "Lightness",
                -1.0,
                1.0,
                0.0,
                "norm",
                "Lightness adjustment (-1..1).",
            ),
            (
                "uniformity",
                "Uniformity",
                0.0,
                1.0,
                0.0,
                "norm",
                "Hue & saturation uniformity toward target (0..1).",
            ),
        ] {
            out.push(ParamDescriptor {
                id: format!("advanced_color.{idx}.{field}"),
                label: format!("Color Slice {num} {label}"),
                group: ParamGroup::Color.as_str().to_owned(),
                kind: "f32".to_owned(),
                description: desc.to_owned(),
                min: Some(min),
                max: Some(max),
                default: Some(json!(def)),
                unit: Some(unit.to_owned()),
                enum_values: None,
            });
        }
    }

    out.push(ParamDescriptor {
        id: "skin_tone.enabled".to_owned(),
        label: "Skin Tone Enabled".to_owned(),
        group: ParamGroup::SkinTone.as_str().to_owned(),
        kind: "bool".to_owned(),
        description: "Enable Capture One Skin Tone Uniformity & Amount.".to_owned(),
        min: None,
        max: None,
        default: Some(json!(false)),
        unit: None,
        enum_values: None,
    });
    for (field, label, min, max, def, unit, desc) in [
        (
            "target_hue",
            "Target Hue",
            0.0,
            360.0,
            24.0,
            "deg",
            "Target skin hue angle (0..360 deg).",
        ),
        (
            "target_sat",
            "Target Sat",
            0.0,
            1.0,
            0.42,
            "norm",
            "Target skin saturation (0..1).",
        ),
        (
            "target_lightness",
            "Target Lightness",
            0.0,
            1.0,
            0.62,
            "norm",
            "Target skin lightness (0..1).",
        ),
        (
            "smoothness",
            "Smoothness",
            0.0,
            1.0,
            0.45,
            "norm",
            "Skin range feather smoothness (0..1).",
        ),
        (
            "uniformity_hue",
            "Uniformity Hue",
            0.0,
            1.0,
            0.0,
            "norm",
            "Pull skin hues toward target hue (0..1).",
        ),
        (
            "uniformity_sat",
            "Uniformity Saturation",
            0.0,
            1.0,
            0.0,
            "norm",
            "Pull skin saturation toward target (0..1).",
        ),
        (
            "uniformity_lightness",
            "Uniformity Lightness",
            0.0,
            1.0,
            0.0,
            "norm",
            "Pull skin lightness toward target (0..1).",
        ),
        (
            "amount_hue",
            "Amount Hue",
            -1.0,
            1.0,
            0.0,
            "norm",
            "Shift selected skin hue (-1..1).",
        ),
        (
            "amount_sat",
            "Amount Saturation",
            -1.0,
            1.0,
            0.0,
            "norm",
            "Shift selected skin saturation (-1..1).",
        ),
        (
            "amount_lightness",
            "Amount Lightness",
            -1.0,
            1.0,
            0.0,
            "norm",
            "Shift selected skin lightness (-1..1).",
        ),
    ] {
        out.push(ParamDescriptor {
            id: format!("skin_tone.{field}"),
            label: format!("Skin Tone {label}"),
            group: ParamGroup::SkinTone.as_str().to_owned(),
            kind: "f32".to_owned(),
            description: desc.to_owned(),
            min: Some(min),
            max: Some(max),
            default: Some(json!(def)),
            unit: Some(unit.to_owned()),
            enum_values: None,
        });
    }

    for (field, label, min, max, def, desc) in [
        (
            "distortion",
            "Distortion",
            -1.0,
            1.0,
            0.0,
            "Radial barrel/pincushion lens distortion correction (-1..1).",
        ),
        (
            "chromatic_aberration",
            "Chromatic Aberration",
            -1.0,
            1.0,
            0.0,
            "Lateral R/B radial chromatic aberration correction (-1..1).",
        ),
        (
            "purple_fringing",
            "Purple Fringing",
            0.0,
            1.0,
            0.0,
            "High-contrast purple/magenta edge fringe suppression (0..1).",
        ),
        (
            "light_falloff",
            "Light Falloff",
            0.0,
            1.0,
            0.0,
            "Optical radial cos^4 corner light falloff compensation (0..1).",
        ),
    ] {
        out.push(ParamDescriptor {
            id: format!("lens.{field}"),
            label: format!("Lens {label}"),
            group: ParamGroup::Lens.as_str().to_owned(),
            kind: "f32".to_owned(),
            description: desc.to_owned(),
            min: Some(min),
            max: Some(max),
            default: Some(json!(def)),
            unit: Some("norm".to_owned()),
            enum_values: None,
        });
    }

    out.push(ParamDescriptor {
        id: "layers".to_owned(),
        label: "Adjustment Layers".to_owned(),
        group: ParamGroup::Layers.as_str().to_owned(),
        kind: "layers".to_owned(),
        description:
            "Capture One-style local Adjustment Layers with composable masks (up to 8 layers)."
                .to_owned(),
        min: None,
        max: None,
        default: Some(json!([])),
        unit: None,
        enum_values: None,
    });
    out.push(ParamDescriptor {
        id: "heal_spots".to_owned(),
        label: "Heal & Clone Spots".to_owned(),
        group: ParamGroup::Layers.as_str().to_owned(),
        kind: "heal_spots".to_owned(),
        description:
            "Spot Heal and Clone retouching stamps in source image UV space (up to 16 spots)."
                .to_owned(),
        min: None,
        max: None,
        default: Some(json!([])),
        unit: None,
        enum_values: None,
    });
    out.push(ParamDescriptor {
        id: "processing_mode".to_owned(),
        label: "Processing Mode".to_owned(),
        group: ParamGroup::FilmNegative.as_str().to_owned(),
        kind: "enum".to_owned(),
        description: "Base Characteristics processing mode (photography vs film_negative)."
            .to_owned(),
        min: None,
        max: None,
        default: Some(json!("photography")),
        unit: None,
        enum_values: Some(
            ProcessingMode::ALL
                .iter()
                .map(|m| m.cli_slug().to_owned())
                .collect(),
        ),
    });
    for (field, label, min, max, def, desc) in [
        (
            "base_r",
            "Base Red",
            0.05,
            2.0,
            0.95,
            "Film negative C-41 base mask red linear reflectance (0.05..2.0).",
        ),
        (
            "base_g",
            "Base Green",
            0.05,
            2.0,
            0.56,
            "Film negative C-41 base mask green linear reflectance (0.05..2.0).",
        ),
        (
            "base_b",
            "Base Blue",
            0.05,
            2.0,
            0.34,
            "Film negative C-41 base mask blue linear reflectance (0.05..2.0).",
        ),
        (
            "density",
            "Density",
            -1.0,
            1.0,
            0.0,
            "Film negative print exposure / density shift (-1..1).",
        ),
        (
            "contrast",
            "Contrast",
            -1.0,
            1.0,
            0.0,
            "Film negative print gamma contrast (-1..1).",
        ),
        (
            "black_clip",
            "Black Clip",
            0.0,
            0.45,
            0.02,
            "Film rebate shadow black point clip (0..0.45).",
        ),
        (
            "white_clip",
            "White Clip",
            0.55,
            1.0,
            0.98,
            "Dense highlight white point clip (0.55..1.0).",
        ),
    ] {
        out.push(ParamDescriptor {
            id: format!("film_negative.{field}"),
            label: format!("Film Negative {label}"),
            group: ParamGroup::FilmNegative.as_str().to_owned(),
            kind: "f32".to_owned(),
            description: desc.to_owned(),
            min: Some(min),
            max: Some(max),
            default: Some(json!(def)),
            unit: Some("norm".to_owned()),
            enum_values: None,
        });
    }

    out
}

/// Searches registered parameters by substring against id, label, group, and description.
pub fn search_params(query: &str) -> Vec<ParamDescriptor> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return all_param_descriptors();
    }
    all_param_descriptors()
        .into_iter()
        .filter(|p| {
            p.id.to_ascii_lowercase().contains(&q)
                || p.label.to_ascii_lowercase().contains(&q)
                || p.group.to_ascii_lowercase().contains(&q)
                || p.description.to_ascii_lowercase().contains(&q)
        })
        .collect()
}

/// Gets a scalar float parameter value by dot-path (e.g. `exposure`, `hsl.orange.saturation`,
/// `shadows_wheel.hue_x`, `levels.in_black`, `bw.red`, `geometry.straighten_deg`).
pub fn get_scalar_by_path(recipe: &DevelopRecipe, path: &str) -> Result<f32, CoreError> {
    let key = canonical_param_key(path);
    if let Some(spec) = SCALAR_PARAMS.iter().find(|s| s.id == key) {
        return Ok((spec.get)(recipe));
    }
    let parts: Vec<&str> = key.split('.').collect();
    match parts.as_slice() {
        ["shadows_wheel", c] => wheel_get(&recipe.shadows_wheel, c, &key),
        ["midtones_wheel", c] => wheel_get(&recipe.midtones_wheel, c, &key),
        ["highlights_wheel", c] => wheel_get(&recipe.highlights_wheel, c, &key),
        ["master_wheel", c] => wheel_get(&recipe.master_wheel, c, &key),
        ["hsl", band, c] => {
            let idx =
                parse_hsl_band_index(band).ok_or_else(|| CoreError::UnknownParam(key.clone()))?;
            match *c {
                "hue" | "h" => Ok(recipe.hsl[idx].hue),
                "saturation" | "sat" | "s" => Ok(recipe.hsl[idx].saturation),
                "luminance" | "lum" | "l" => Ok(recipe.hsl[idx].luminance),
                _ => Err(CoreError::UnknownParam(key)),
            }
        }
        ["levels", c] => match *c {
            "in_black" => Ok(recipe.levels.in_black),
            "in_mid" => Ok(recipe.levels.in_mid),
            "in_white" => Ok(recipe.levels.in_white),
            "out_black" => Ok(recipe.levels.out_black),
            "out_white" => Ok(recipe.levels.out_white),
            _ => Err(CoreError::UnknownParam(key)),
        },
        ["levels", ch, c] => {
            let channel = match *ch {
                "red" | "r" => &recipe.levels.red,
                "green" | "g" => &recipe.levels.green,
                "blue" | "b" => &recipe.levels.blue,
                _ => return Err(CoreError::UnknownParam(key)),
            };
            match *c {
                "in_black" => Ok(channel.in_black),
                "in_mid" => Ok(channel.in_mid),
                "in_white" => Ok(channel.in_white),
                "out_black" => Ok(channel.out_black),
                "out_white" => Ok(channel.out_white),
                _ => Err(CoreError::UnknownParam(key)),
            }
        }
        ["bw", c] => match *c {
            "red" => Ok(recipe.bw.red),
            "yellow" => Ok(recipe.bw.yellow),
            "green" => Ok(recipe.bw.green),
            "cyan" => Ok(recipe.bw.cyan),
            "blue" => Ok(recipe.bw.blue),
            "magenta" => Ok(recipe.bw.magenta),
            "split_shadow_hue" => Ok(recipe.bw.split_shadow_hue),
            "split_shadow_sat" => Ok(recipe.bw.split_shadow_sat),
            "split_highlight_hue" => Ok(recipe.bw.split_highlight_hue),
            "split_highlight_sat" => Ok(recipe.bw.split_highlight_sat),
            _ => Err(CoreError::UnknownParam(key)),
        },
        ["advanced_color", idx_str, c] => {
            let idx =
                parse_slice_index(idx_str).ok_or_else(|| CoreError::UnknownParam(key.clone()))?;
            let sl = &recipe.advanced_color[idx];
            match *c {
                "target_hue" => Ok(sl.target_hue),
                "target_sat" => Ok(sl.target_sat),
                "hue_range" => Ok(sl.hue_range),
                "sat_range" => Ok(sl.sat_range),
                "smoothness" => Ok(sl.smoothness),
                "hue_shift" | "hue" => Ok(sl.hue_shift),
                "saturation" | "sat" => Ok(sl.saturation),
                "lightness" | "lum" | "luma" => Ok(sl.lightness),
                "uniformity" => Ok(sl.uniformity),
                _ => Err(CoreError::UnknownParam(key)),
            }
        }
        ["skin_tone", c] => match *c {
            "target_hue" => Ok(recipe.skin_tone.target_hue),
            "target_sat" => Ok(recipe.skin_tone.target_sat),
            "target_lightness" => Ok(recipe.skin_tone.target_lightness),
            "smoothness" => Ok(recipe.skin_tone.smoothness),
            "uniformity_hue" => Ok(recipe.skin_tone.uniformity_hue),
            "uniformity_sat" => Ok(recipe.skin_tone.uniformity_sat),
            "uniformity_lightness" => Ok(recipe.skin_tone.uniformity_lightness),
            "amount_hue" => Ok(recipe.skin_tone.amount_hue),
            "amount_sat" => Ok(recipe.skin_tone.amount_sat),
            "amount_lightness" => Ok(recipe.skin_tone.amount_lightness),
            _ => Err(CoreError::UnknownParam(key)),
        },
        ["lens", c] => match *c {
            "distortion" => Ok(recipe.lens.distortion),
            "chromatic_aberration" => Ok(recipe.lens.chromatic_aberration),
            "purple_fringing" => Ok(recipe.lens.purple_fringing),
            "light_falloff" => Ok(recipe.lens.light_falloff),
            _ => Err(CoreError::UnknownParam(key)),
        },
        ["geometry", "straighten_deg"] => Ok(recipe.geometry.straighten_deg),
        ["geometry", "keystone_vertical"] => Ok(recipe.geometry.keystone_vertical),
        ["geometry", "keystone_horizontal"] => Ok(recipe.geometry.keystone_horizontal),
        ["geometry", "keystone_aspect"] => Ok(recipe.geometry.keystone_aspect),
        ["geometry", "keystone_amount"] => Ok(recipe.geometry.keystone_amount),
        ["film_negative", c] => match *c {
            "base_r" => Ok(recipe.film_negative.base_r),
            "base_g" => Ok(recipe.film_negative.base_g),
            "base_b" => Ok(recipe.film_negative.base_b),
            "density" => Ok(recipe.film_negative.density),
            "contrast" => Ok(recipe.film_negative.contrast),
            "black_clip" => Ok(recipe.film_negative.black_clip),
            "white_clip" => Ok(recipe.film_negative.white_clip),
            _ => Err(CoreError::UnknownParam(key)),
        },
        _ => Err(CoreError::UnknownParam(key)),
    }
}

/// Sets a parameter by dot-path (validating bounds and finiteness). Supports floats,
/// booleans (`bw.enabled`, `geometry.flip_h`, `geometry.flip_v`), integers (`geometry.rotate_90`),
/// and enums (`geometry.aspect_preset`, `geometry.grid`).
pub fn set_param_by_str(
    recipe: &mut DevelopRecipe,
    path: &str,
    raw_value: &str,
    delta: bool,
    src_dims: Option<[u32; 2]>,
) -> Result<(), CoreError> {
    let key = canonical_param_key(path);
    let val_trim = raw_value.trim();

    if let Some(spec) = SCALAR_PARAMS.iter().find(|s| s.id == key) {
        let parsed = parse_finite_f32(val_trim, &key)?;
        let target = if delta {
            (spec.get)(recipe) + parsed
        } else {
            parsed
        };
        if !delta && (target < spec.min || target > spec.max) {
            return Err(CoreError::Validation(format!(
                "{} must be within {}..={} (got {})",
                spec.id, spec.min, spec.max, target
            )));
        }
        (spec.set)(recipe, target.clamp(spec.min, spec.max));
        if spec.id == "temperature" || spec.id == "tint" {
            recipe.wb_preset = WbPreset::Custom;
        }
        recipe.preset_name = None;
        return Ok(());
    }

    let parts: Vec<&str> = key.split('.').collect();
    match parts.as_slice() {
        ["film_curve"] => {
            if delta {
                return Err(CoreError::Validation(
                    "film_curve does not support +=/-=".to_owned(),
                ));
            }
            recipe.film_curve = FilmCurve::from_cli_str(val_trim).ok_or_else(|| {
                CoreError::Validation(format!(
                    "Unknown film_curve '{val_trim}' (expected film_standard|film_extra_shadow|film_high_contrast|linear_response)"
                ))
            })?;
        }
        ["processing_mode"] => {
            if delta {
                return Err(CoreError::Validation(
                    "processing_mode does not support +=/-=".to_owned(),
                ));
            }
            recipe.processing_mode = ProcessingMode::from_cli_str(val_trim).ok_or_else(|| {
                CoreError::Validation(format!(
                    "Unknown processing_mode '{val_trim}' (expected photography|film_negative)"
                ))
            })?;
        }
        ["film_negative", c] => {
            let v = parse_finite_f32(val_trim, &key)?;
            match *c {
                "base_r" => {
                    set_clamped_f32(&mut recipe.film_negative.base_r, v, delta, 0.05, 2.0, &key)?;
                }
                "base_g" => {
                    set_clamped_f32(&mut recipe.film_negative.base_g, v, delta, 0.05, 2.0, &key)?;
                }
                "base_b" => {
                    set_clamped_f32(&mut recipe.film_negative.base_b, v, delta, 0.05, 2.0, &key)?;
                }
                "density" => {
                    set_clamped_f32(&mut recipe.film_negative.density, v, delta, -1.0, 1.0, &key)?;
                }
                "contrast" => {
                    set_clamped_f32(
                        &mut recipe.film_negative.contrast,
                        v,
                        delta,
                        -1.0,
                        1.0,
                        &key,
                    )?;
                }
                "black_clip" => {
                    set_clamped_f32(
                        &mut recipe.film_negative.black_clip,
                        v,
                        delta,
                        0.0,
                        0.45,
                        &key,
                    )?;
                }
                "white_clip" => {
                    set_clamped_f32(
                        &mut recipe.film_negative.white_clip,
                        v,
                        delta,
                        0.55,
                        1.0,
                        &key,
                    )?;
                }
                _ => return Err(CoreError::UnknownParam(key)),
            }
            recipe.film_negative = recipe.film_negative.sanitized();
        }
        ["wb_preset"] => {
            if delta {
                return Err(CoreError::Validation(
                    "wb_preset does not support +=/-=".to_owned(),
                ));
            }
            let preset = WbPreset::from_cli_str(val_trim).ok_or_else(|| {
                CoreError::Validation(format!(
                    "Unknown wb_preset '{val_trim}' (expected as_shot|daylight|cloudy|shade|tungsten|fluorescent|flash|custom)"
                ))
            })?;
            recipe.apply_wb_preset(preset);
        }
        ["clarity_method"] => {
            if delta {
                return Err(CoreError::Validation(
                    "clarity_method does not support +=/-=".to_owned(),
                ));
            }
            recipe.clarity_method = ClarityMethod::from_cli_str(val_trim).ok_or_else(|| {
                CoreError::Validation(format!(
                    "Unknown clarity_method '{val_trim}' (expected natural|punch|neutral|classic)"
                ))
            })?;
        }
        ["grain_type"] => {
            if delta {
                return Err(CoreError::Validation(
                    "grain_type does not support +=/-=".to_owned(),
                ));
            }
            recipe.grain_type = GrainType::from_cli_str(val_trim).ok_or_else(|| {
                CoreError::Validation(format!(
                    "Unknown grain_type '{val_trim}' (expected fine|silver_rich|soft|cubic|tabular|harsh)"
                ))
            })?;
        }
        ["vignette_method"] => {
            if delta {
                return Err(CoreError::Validation(
                    "vignette_method does not support +=/-=".to_owned(),
                ));
            }
            recipe.vignette_method = VignetteMethod::from_cli_str(val_trim).ok_or_else(|| {
                CoreError::Validation(format!(
                    "Unknown vignette_method '{val_trim}' (expected elliptic_on_crop|circular_on_crop|circular)"
                ))
            })?;
        }
        ["shadows_wheel", c] => {
            let v = parse_finite_f32(val_trim, &key)?;
            wheel_set(&mut recipe.shadows_wheel, c, v, delta, &key)?;
            recipe.shadows_wheel = recipe.shadows_wheel.clamped();
        }
        ["midtones_wheel", c] => {
            let v = parse_finite_f32(val_trim, &key)?;
            wheel_set(&mut recipe.midtones_wheel, c, v, delta, &key)?;
            recipe.midtones_wheel = recipe.midtones_wheel.clamped();
        }
        ["highlights_wheel", c] => {
            let v = parse_finite_f32(val_trim, &key)?;
            wheel_set(&mut recipe.highlights_wheel, c, v, delta, &key)?;
            recipe.highlights_wheel = recipe.highlights_wheel.clamped();
        }
        ["master_wheel", c] => {
            let v = parse_finite_f32(val_trim, &key)?;
            wheel_set(&mut recipe.master_wheel, c, v, delta, &key)?;
            recipe.master_wheel = recipe.master_wheel.clamped();
        }
        ["hsl", band, c] => {
            let idx =
                parse_hsl_band_index(band).ok_or_else(|| CoreError::UnknownParam(key.clone()))?;
            let v = parse_finite_f32(val_trim, &key)?;
            let slot = match *c {
                "hue" | "h" => &mut recipe.hsl[idx].hue,
                "saturation" | "sat" | "s" => &mut recipe.hsl[idx].saturation,
                "luminance" | "lum" | "l" => &mut recipe.hsl[idx].luminance,
                _ => return Err(CoreError::UnknownParam(key)),
            };
            let target = if delta { *slot + v } else { v };
            if !delta && !(-1.0..=1.0).contains(&target) {
                return Err(CoreError::Validation(format!(
                    "{key} must be within -1.0..=1.0 (got {target})"
                )));
            }
            *slot = target.clamp(-1.0, 1.0);
        }
        ["levels", c] => {
            let v = parse_finite_f32(val_trim, &key)?;
            let (slot, min, max) = match *c {
                "in_black" => (&mut recipe.levels.in_black, 0.0_f32, 0.95_f32),
                "in_mid" => (&mut recipe.levels.in_mid, -1.0_f32, 1.0_f32),
                "in_white" => (&mut recipe.levels.in_white, 0.05_f32, 1.0_f32),
                "out_black" => (&mut recipe.levels.out_black, 0.0_f32, 1.0_f32),
                "out_white" => (&mut recipe.levels.out_white, 0.0_f32, 1.0_f32),
                _ => return Err(CoreError::UnknownParam(key)),
            };
            let target = if delta { *slot + v } else { v };
            if !delta && (target < min || target > max) {
                return Err(CoreError::Validation(format!(
                    "{key} must be within {min}..={max} (got {target})"
                )));
            }
            *slot = target.clamp(min, max);
            recipe.levels = recipe.levels.sanitized();
        }
        ["levels", ch, c] => {
            let v = parse_finite_f32(val_trim, &key)?;
            let channel = match *ch {
                "red" | "r" => &mut recipe.levels.red,
                "green" | "g" => &mut recipe.levels.green,
                "blue" | "b" => &mut recipe.levels.blue,
                _ => return Err(CoreError::UnknownParam(key)),
            };
            let (slot, min, max) = match *c {
                "in_black" => (&mut channel.in_black, 0.0_f32, 0.95_f32),
                "in_mid" => (&mut channel.in_mid, -1.0_f32, 1.0_f32),
                "in_white" => (&mut channel.in_white, 0.05_f32, 1.0_f32),
                "out_black" => (&mut channel.out_black, 0.0_f32, 1.0_f32),
                "out_white" => (&mut channel.out_white, 0.0_f32, 1.0_f32),
                _ => return Err(CoreError::UnknownParam(key)),
            };
            let target = if delta { *slot + v } else { v };
            if !delta && (target < min || target > max) {
                return Err(CoreError::Validation(format!(
                    "{key} must be within {min}..={max} (got {target})"
                )));
            }
            *slot = target.clamp(min, max);
            recipe.levels = recipe.levels.sanitized();
        }
        ["bw", "enabled"] => {
            if delta {
                return Err(CoreError::Validation(
                    "bw.enabled does not support +=/-=".to_owned(),
                ));
            }
            recipe.bw.enabled = parse_bool_str(val_trim, &key)?;
        }
        ["bw", c] => {
            let v = parse_finite_f32(val_trim, &key)?;
            let (slot, min, max) = match *c {
                "red" => (&mut recipe.bw.red, -1.0_f32, 1.0_f32),
                "yellow" => (&mut recipe.bw.yellow, -1.0_f32, 1.0_f32),
                "green" => (&mut recipe.bw.green, -1.0_f32, 1.0_f32),
                "cyan" => (&mut recipe.bw.cyan, -1.0_f32, 1.0_f32),
                "blue" => (&mut recipe.bw.blue, -1.0_f32, 1.0_f32),
                "magenta" => (&mut recipe.bw.magenta, -1.0_f32, 1.0_f32),
                "split_shadow_hue" => (&mut recipe.bw.split_shadow_hue, 0.0_f32, 360.0_f32),
                "split_shadow_sat" => (&mut recipe.bw.split_shadow_sat, 0.0_f32, 1.0_f32),
                "split_highlight_hue" => (&mut recipe.bw.split_highlight_hue, 0.0_f32, 360.0_f32),
                "split_highlight_sat" => (&mut recipe.bw.split_highlight_sat, 0.0_f32, 1.0_f32),
                _ => return Err(CoreError::UnknownParam(key)),
            };
            let target = if delta { *slot + v } else { v };
            if !delta && (target < min || target > max) {
                return Err(CoreError::Validation(format!(
                    "{key} must be within {min}..={max} (got {target})"
                )));
            }
            *slot = target.clamp(min, max);
        }
        ["curves", ch] => {
            if delta {
                return Err(CoreError::Validation(
                    "curves.* does not support +=/-= (pass control points or `reset`)".to_owned(),
                ));
            }
            let pts = parse_curve_points_str(val_trim, &key)?;
            match *ch {
                "rgb" => recipe.curves.rgb = pts,
                "luma" | "master" => recipe.curves.luma = pts,
                "red" | "r" => recipe.curves.red = pts,
                "green" | "g" => recipe.curves.green = pts,
                "blue" | "b" => recipe.curves.blue = pts,
                _ => return Err(CoreError::UnknownParam(key)),
            }
        }
        ["geometry", "crop"] => {
            if delta {
                return Err(CoreError::Validation(
                    "geometry.crop does not support +=/-=".to_owned(),
                ));
            }
            let rect = parse_crop_rect_str(val_trim, &key)?;
            recipe.geometry.crop = rect;
            recipe.geometry.crop = recipe.geometry.sanitized_crop();
        }
        ["geometry", "straighten_deg"] => {
            let v = parse_finite_f32(val_trim, &key)?;
            let target = if delta {
                recipe.geometry.straighten_deg + v
            } else {
                v
            };
            if !delta && !(-45.0..=45.0).contains(&target) {
                return Err(CoreError::Validation(format!(
                    "{key} must be within -45.0..=45.0 (got {target})"
                )));
            }
            recipe.geometry.straighten_deg = target.clamp(-45.0, 45.0);
        }
        ["geometry", "rotate_90"] => {
            let turns: i8 = val_trim.parse().map_err(|_| {
                CoreError::Validation(format!("Invalid integer for {key}: {val_trim}"))
            })?;
            if delta {
                recipe.geometry.rotate_90 = (recipe.geometry.rotate_90 + turns).rem_euclid(4);
            } else {
                recipe.geometry.rotate_90 = turns.rem_euclid(4);
            }
        }
        ["geometry", "flip_h"] => {
            recipe.geometry.flip_h = parse_bool_str(val_trim, &key)?;
        }
        ["geometry", "flip_v"] => {
            recipe.geometry.flip_v = parse_bool_str(val_trim, &key)?;
        }
        ["geometry", "aspect_preset"] => {
            let aspect = CropAspect::from_cli_str(val_trim).ok_or_else(|| {
                CoreError::Validation(format!("Unknown aspect preset '{val_trim}'"))
            })?;
            if let Some([w, h]) = src_dims {
                recipe.geometry.apply_aspect_preset(aspect, w, h);
            } else {
                recipe.geometry.aspect_preset = aspect;
            }
        }
        ["advanced_color", idx_str, "enabled"] => {
            if delta {
                return Err(CoreError::Validation(
                    "advanced_color.*.enabled does not support +=/-=".to_owned(),
                ));
            }
            let idx =
                parse_slice_index(idx_str).ok_or_else(|| CoreError::UnknownParam(key.clone()))?;
            recipe.advanced_color[idx].enabled = parse_bool_str(val_trim, &key)?;
        }
        ["advanced_color", idx_str, c] => {
            let idx =
                parse_slice_index(idx_str).ok_or_else(|| CoreError::UnknownParam(key.clone()))?;
            let v = parse_finite_f32(val_trim, &key)?;
            let sl = &mut recipe.advanced_color[idx];
            let (slot, min, max) = match *c {
                "target_hue" => (&mut sl.target_hue, 0.0_f32, 360.0_f32),
                "target_sat" => (&mut sl.target_sat, 0.0_f32, 1.0_f32),
                "hue_range" => (&mut sl.hue_range, 5.0_f32, 180.0_f32),
                "sat_range" => (&mut sl.sat_range, 0.05_f32, 1.0_f32),
                "smoothness" => (&mut sl.smoothness, 0.0_f32, 1.0_f32),
                "hue_shift" | "hue" => (&mut sl.hue_shift, -1.0_f32, 1.0_f32),
                "saturation" | "sat" => (&mut sl.saturation, -1.0_f32, 1.0_f32),
                "lightness" | "lum" | "luma" => (&mut sl.lightness, -1.0_f32, 1.0_f32),
                "uniformity" => (&mut sl.uniformity, 0.0_f32, 1.0_f32),
                _ => return Err(CoreError::UnknownParam(key)),
            };
            let target = if delta { *slot + v } else { v };
            if !delta && (target < min || target > max) {
                return Err(CoreError::Validation(format!(
                    "{key} must be within {min}..={max} (got {target})"
                )));
            }
            *slot = target.clamp(min, max);
        }
        ["skin_tone", "enabled"] => {
            if delta {
                return Err(CoreError::Validation(
                    "skin_tone.enabled does not support +=/-=".to_owned(),
                ));
            }
            recipe.skin_tone.enabled = parse_bool_str(val_trim, &key)?;
        }
        ["skin_tone", c] => {
            let v = parse_finite_f32(val_trim, &key)?;
            let (slot, min, max) = match *c {
                "target_hue" => (&mut recipe.skin_tone.target_hue, 0.0_f32, 360.0_f32),
                "target_sat" => (&mut recipe.skin_tone.target_sat, 0.0_f32, 1.0_f32),
                "target_lightness" => (&mut recipe.skin_tone.target_lightness, 0.0_f32, 1.0_f32),
                "smoothness" => (&mut recipe.skin_tone.smoothness, 0.0_f32, 1.0_f32),
                "uniformity_hue" => (&mut recipe.skin_tone.uniformity_hue, 0.0_f32, 1.0_f32),
                "uniformity_sat" => (&mut recipe.skin_tone.uniformity_sat, 0.0_f32, 1.0_f32),
                "uniformity_lightness" => {
                    (&mut recipe.skin_tone.uniformity_lightness, 0.0_f32, 1.0_f32)
                }
                "amount_hue" => (&mut recipe.skin_tone.amount_hue, -1.0_f32, 1.0_f32),
                "amount_sat" => (&mut recipe.skin_tone.amount_sat, -1.0_f32, 1.0_f32),
                "amount_lightness" => (&mut recipe.skin_tone.amount_lightness, -1.0_f32, 1.0_f32),
                _ => return Err(CoreError::UnknownParam(key)),
            };
            let target = if delta { *slot + v } else { v };
            if !delta && (target < min || target > max) {
                return Err(CoreError::Validation(format!(
                    "{key} must be within {min}..={max} (got {target})"
                )));
            }
            *slot = target.clamp(min, max);
        }
        ["lens", c] => {
            let v = parse_finite_f32(val_trim, &key)?;
            let (slot, min, max) = match *c {
                "distortion" => (&mut recipe.lens.distortion, -1.0_f32, 1.0_f32),
                "chromatic_aberration" => {
                    (&mut recipe.lens.chromatic_aberration, -1.0_f32, 1.0_f32)
                }
                "purple_fringing" => (&mut recipe.lens.purple_fringing, 0.0_f32, 1.0_f32),
                "light_falloff" => (&mut recipe.lens.light_falloff, 0.0_f32, 1.0_f32),
                _ => return Err(CoreError::UnknownParam(key)),
            };
            let target = if delta { *slot + v } else { v };
            if !delta && (target < min || target > max) {
                return Err(CoreError::Validation(format!(
                    "{key} must be within {min}..={max} (got {target})"
                )));
            }
            *slot = target.clamp(min, max);
        }
        ["geometry", "keystone_vertical"] => {
            set_clamped_f32(
                &mut recipe.geometry.keystone_vertical,
                parse_finite_f32(val_trim, &key)?,
                delta,
                -1.0,
                1.0,
                &key,
            )?;
        }
        ["geometry", "keystone_horizontal"] => {
            set_clamped_f32(
                &mut recipe.geometry.keystone_horizontal,
                parse_finite_f32(val_trim, &key)?,
                delta,
                -1.0,
                1.0,
                &key,
            )?;
        }
        ["geometry", "keystone_aspect"] => {
            set_clamped_f32(
                &mut recipe.geometry.keystone_aspect,
                parse_finite_f32(val_trim, &key)?,
                delta,
                -0.5,
                1.0,
                &key,
            )?;
        }
        ["geometry", "keystone_amount"] => {
            set_clamped_f32(
                &mut recipe.geometry.keystone_amount,
                parse_finite_f32(val_trim, &key)?,
                delta,
                0.0,
                1.0,
                &key,
            )?;
        }
        ["geometry", "grid"] => {
            recipe.geometry.grid = CompositionGrid::from_cli_str(val_trim)
                .ok_or_else(|| CoreError::Validation(format!("Unknown grid mode '{val_trim}'")))?;
        }
        _ => return Err(CoreError::UnknownParam(key)),
    }

    recipe.preset_name = None;
    Ok(())
}

/// Resets a tool group (or `"all"`) back to neutral defaults.
pub fn reset_recipe_group(recipe: &mut DevelopRecipe, group: &str) -> Result<(), CoreError> {
    let d = DevelopRecipe::default();
    match group.trim().to_ascii_lowercase().as_str() {
        "all" => *recipe = d,
        "tone" => {
            recipe.film_curve = d.film_curve;
            recipe.exposure = d.exposure;
            recipe.contrast = d.contrast;
            recipe.brightness = d.brightness;
            recipe.highlights = d.highlights;
            recipe.shadows = d.shadows;
            recipe.whites = d.whites;
            recipe.blacks = d.blacks;
            recipe.levels = d.levels;
        }
        "wb" | "white_balance" => {
            recipe.wb_preset = d.wb_preset;
            recipe.temperature = d.temperature;
            recipe.tint = d.tint;
        }
        "color" => {
            recipe.vibrance = d.vibrance;
            recipe.saturation = d.saturation;
            recipe.shadows_wheel = d.shadows_wheel;
            recipe.midtones_wheel = d.midtones_wheel;
            recipe.highlights_wheel = d.highlights_wheel;
            recipe.master_wheel = d.master_wheel;
            recipe.hsl = d.hsl;
            recipe.advanced_color = d.advanced_color;
            recipe.skin_tone = d.skin_tone;
            recipe.bw = d.bw;
        }
        "wheels" => {
            recipe.shadows_wheel = d.shadows_wheel;
            recipe.midtones_wheel = d.midtones_wheel;
            recipe.highlights_wheel = d.highlights_wheel;
            recipe.master_wheel = d.master_wheel;
        }
        "hsl" => recipe.hsl = d.hsl,
        "advanced_color" => recipe.advanced_color = d.advanced_color,
        "skin_tone" | "skin" => recipe.skin_tone = d.skin_tone,
        "levels" => recipe.levels = d.levels,
        "curves" => recipe.curves = d.curves,
        "detail" => {
            recipe.clarity = d.clarity;
            recipe.clarity_method = d.clarity_method;
            recipe.structure = d.structure;
            recipe.dehaze = d.dehaze;
            recipe.sharpen = d.sharpen;
            recipe.sharpen_radius = d.sharpen_radius;
            recipe.sharpen_threshold = d.sharpen_threshold;
            recipe.sharpen_halo = d.sharpen_halo;
            recipe.noise_reduction = d.noise_reduction;
            recipe.nr_color = d.nr_color;
        }
        "bw" => recipe.bw = d.bw,
        "effects" => {
            recipe.vignette = d.vignette;
            recipe.vignette_method = d.vignette_method;
            recipe.grain = d.grain;
            recipe.grain_type = d.grain_type;
            recipe.grain_size = d.grain_size;
        }
        "lens" => recipe.lens = d.lens,
        "geometry" | "crop" => recipe.geometry = d.geometry,
        "layers" => {
            recipe.layers = d.layers;
            recipe.heal_spots = d.heal_spots;
        }
        "heal" | "heal_spots" => recipe.heal_spots = d.heal_spots,
        "film_negative" => {
            recipe.processing_mode = d.processing_mode;
            recipe.film_negative = d.film_negative;
        }
        other => {
            return Err(CoreError::Validation(format!(
                "Unknown reset group '{other}'. Valid: all, tone, wb, color, wheels, hsl, advanced_color, skin_tone, levels, curves, detail, bw, effects, lens, geometry, layers, heal, film_negative"
            )));
        }
    }
    if !recipe.is_neutral() {
        recipe.preset_name = None;
    }
    Ok(())
}

/// Canonical Capture One Selective Adjustments Clipboard tool groups `(id, display_label)`.
pub const ALL_CLIPBOARD_GROUPS: &[(&str, &str)] = &[
    ("wb", "White Balance"),
    ("tone", "Exposure & Tone"),
    ("levels", "Levels"),
    ("curves", "Tone Curve"),
    ("wheels", "Color Balance"),
    ("color", "Color Editor"),
    ("skin_tone", "Skin Tone"),
    ("bw", "Black & White"),
    ("detail", "Detail"),
    ("effects", "Effects"),
    ("lens", "Lens"),
    ("geometry", "Geometry"),
    ("layers", "Layers & Retouch"),
    ("film_negative", "Film Negative"),
];

/// Returns the subset of [`ALL_CLIPBOARD_GROUPS`] whose parameters differ from Neutral defaults
/// (Capture One's **Autoselect Adjusted** behavior).
pub fn modified_recipe_groups(recipe: &DevelopRecipe) -> Vec<&'static str> {
    let d = DevelopRecipe::default();
    let mut out = Vec::new();

    if recipe.wb_preset != d.wb_preset
        || (recipe.temperature - d.temperature).abs() > 1e-6
        || (recipe.tint - d.tint).abs() > 1e-6
    {
        out.push("wb");
    }
    if recipe.film_curve != d.film_curve
        || (recipe.exposure - d.exposure).abs() > 1e-6
        || (recipe.contrast - d.contrast).abs() > 1e-6
        || (recipe.brightness - d.brightness).abs() > 1e-6
        || (recipe.highlights - d.highlights).abs() > 1e-6
        || (recipe.shadows - d.shadows).abs() > 1e-6
        || (recipe.whites - d.whites).abs() > 1e-6
        || (recipe.blacks - d.blacks).abs() > 1e-6
    {
        out.push("tone");
    }
    if !recipe.levels.is_identity() {
        out.push("levels");
    }
    if !recipe.curves.is_identity() {
        out.push("curves");
    }
    if !recipe.shadows_wheel.is_neutral()
        || !recipe.midtones_wheel.is_neutral()
        || !recipe.highlights_wheel.is_neutral()
        || !recipe.master_wheel.is_neutral()
    {
        out.push("wheels");
    }
    if (recipe.vibrance - d.vibrance).abs() > 1e-6
        || (recipe.saturation - d.saturation).abs() > 1e-6
        || recipe.hsl.iter().any(|b| !b.is_neutral())
        || recipe.advanced_color.iter().any(|s| !s.is_neutral())
    {
        out.push("color");
    }
    if !recipe.skin_tone.is_neutral() {
        out.push("skin_tone");
    }
    if !recipe.bw.is_neutral() {
        out.push("bw");
    }
    if (recipe.clarity - d.clarity).abs() > 1e-6
        || recipe.clarity_method != d.clarity_method
        || (recipe.structure - d.structure).abs() > 1e-6
        || (recipe.dehaze - d.dehaze).abs() > 1e-6
        || (recipe.sharpen - d.sharpen).abs() > 1e-6
        || (recipe.sharpen_radius - d.sharpen_radius).abs() > 1e-6
        || (recipe.sharpen_threshold - d.sharpen_threshold).abs() > 1e-6
        || (recipe.sharpen_halo - d.sharpen_halo).abs() > 1e-6
        || (recipe.noise_reduction - d.noise_reduction).abs() > 1e-6
        || (recipe.nr_color - d.nr_color).abs() > 1e-6
    {
        out.push("detail");
    }
    if (recipe.vignette - d.vignette).abs() > 1e-6
        || recipe.vignette_method != d.vignette_method
        || (recipe.grain - d.grain).abs() > 1e-6
        || recipe.grain_type != d.grain_type
        || (recipe.grain_size - d.grain_size).abs() > 1e-6
    {
        out.push("effects");
    }
    if !recipe.lens.is_identity() {
        out.push("lens");
    }
    if !recipe.geometry.is_identity() || recipe.geometry.aspect_preset != d.geometry.aspect_preset {
        out.push("geometry");
    }
    if !recipe.layers.is_empty() || !recipe.heal_spots.is_empty() {
        out.push("layers");
    }
    if recipe.processing_mode != ProcessingMode::Photography || !recipe.film_negative.is_default() {
        out.push("film_negative");
    }

    out
}

/// Copies only the requested tool groups from `src` onto `dst`.
pub fn copy_recipe_groups(
    dst: &mut DevelopRecipe,
    src: &DevelopRecipe,
    groups: &[String],
) -> Result<(), CoreError> {
    if groups.is_empty() {
        *dst = src.clone();
        return Ok(());
    }
    for group in groups {
        match group.trim().to_ascii_lowercase().as_str() {
            "all" => *dst = src.clone(),
            "tone" => {
                dst.film_curve = src.film_curve;
                dst.exposure = src.exposure;
                dst.contrast = src.contrast;
                dst.brightness = src.brightness;
                dst.highlights = src.highlights;
                dst.shadows = src.shadows;
                dst.whites = src.whites;
                dst.blacks = src.blacks;
                dst.levels = src.levels;
            }
            "wb" | "white_balance" => {
                dst.wb_preset = src.wb_preset;
                dst.temperature = src.temperature;
                dst.tint = src.tint;
            }
            "color" => {
                dst.vibrance = src.vibrance;
                dst.saturation = src.saturation;
                dst.shadows_wheel = src.shadows_wheel;
                dst.midtones_wheel = src.midtones_wheel;
                dst.highlights_wheel = src.highlights_wheel;
                dst.master_wheel = src.master_wheel;
                dst.hsl = src.hsl;
                dst.advanced_color = src.advanced_color;
                dst.skin_tone = src.skin_tone;
                dst.bw = src.bw;
            }
            "wheels" => {
                dst.shadows_wheel = src.shadows_wheel;
                dst.midtones_wheel = src.midtones_wheel;
                dst.highlights_wheel = src.highlights_wheel;
                dst.master_wheel = src.master_wheel;
            }
            "hsl" => dst.hsl = src.hsl,
            "advanced_color" => dst.advanced_color = src.advanced_color,
            "skin_tone" | "skin" => dst.skin_tone = src.skin_tone,
            "levels" => dst.levels = src.levels,
            "curves" => dst.curves = src.curves.clone(),
            "detail" => {
                dst.clarity = src.clarity;
                dst.clarity_method = src.clarity_method;
                dst.structure = src.structure;
                dst.dehaze = src.dehaze;
                dst.sharpen = src.sharpen;
                dst.sharpen_radius = src.sharpen_radius;
                dst.sharpen_threshold = src.sharpen_threshold;
                dst.sharpen_halo = src.sharpen_halo;
                dst.noise_reduction = src.noise_reduction;
                dst.nr_color = src.nr_color;
            }
            "bw" => dst.bw = src.bw,
            "effects" => {
                dst.vignette = src.vignette;
                dst.vignette_method = src.vignette_method;
                dst.grain = src.grain;
                dst.grain_type = src.grain_type;
                dst.grain_size = src.grain_size;
            }
            "lens" => dst.lens = src.lens,
            "geometry" | "crop" => dst.geometry = src.geometry,
            "layers" => {
                dst.layers = src.layers.clone();
                dst.heal_spots = src.heal_spots.clone();
            }
            "heal" | "heal_spots" => dst.heal_spots = src.heal_spots.clone(),
            "film_negative" => {
                dst.processing_mode = src.processing_mode;
                dst.film_negative = src.film_negative;
            }
            other => {
                return Err(CoreError::Validation(format!(
                    "Unknown tool group '{other}'. Valid: all, tone, wb, color, wheels, hsl, advanced_color, skin_tone, levels, curves, detail, bw, effects, lens, geometry, layers, heal, film_negative"
                )));
            }
        }
    }
    dst.preset_name = None;
    Ok(())
}

/// Returns a concise JSON object containing ONLY non-default fields of `recipe`.
pub fn recipe_non_default_map(recipe: &DevelopRecipe) -> Map<String, Value> {
    let default = DevelopRecipe::default();
    let mut map = Map::new();
    for spec in SCALAR_PARAMS {
        let cur = (spec.get)(recipe);
        if (cur - spec.default).abs() > 1e-5 {
            map.insert(spec.id.to_owned(), json!(round4(cur)));
        }
    }
    if recipe.film_curve != default.film_curve {
        map.insert("film_curve".to_owned(), json!(recipe.film_curve));
    }
    if recipe.wb_preset != default.wb_preset {
        map.insert("wb_preset".to_owned(), json!(recipe.wb_preset));
    }
    if recipe.clarity_method != default.clarity_method {
        map.insert("clarity_method".to_owned(), json!(recipe.clarity_method));
    }
    if recipe.grain_type != default.grain_type {
        map.insert("grain_type".to_owned(), json!(recipe.grain_type));
    }
    if recipe.vignette_method != default.vignette_method {
        map.insert("vignette_method".to_owned(), json!(recipe.vignette_method));
    }
    if !recipe.shadows_wheel.is_neutral() {
        map.insert("shadows_wheel".to_owned(), json!(recipe.shadows_wheel));
    }
    if !recipe.midtones_wheel.is_neutral() {
        map.insert("midtones_wheel".to_owned(), json!(recipe.midtones_wheel));
    }
    if !recipe.highlights_wheel.is_neutral() {
        map.insert(
            "highlights_wheel".to_owned(),
            json!(recipe.highlights_wheel),
        );
    }
    if !recipe.master_wheel.is_neutral() {
        map.insert("master_wheel".to_owned(), json!(recipe.master_wheel));
    }
    if !recipe.curves.is_identity() {
        map.insert("curves".to_owned(), json!(recipe.curves));
    }
    let mut hsl_map = Map::new();
    for (idx, band) in recipe.hsl.iter().enumerate() {
        if !band.is_neutral() {
            hsl_map.insert(HSL_BAND_NAMES[idx].to_ascii_lowercase(), json!(band));
        }
    }
    if !hsl_map.is_empty() {
        map.insert("hsl".to_owned(), Value::Object(hsl_map));
    }
    let mut adv_map = Map::new();
    for (idx, slice) in recipe.advanced_color.iter().enumerate() {
        if !slice.is_neutral() {
            adv_map.insert(idx.to_string(), json!(slice));
        }
    }
    if !adv_map.is_empty() {
        map.insert("advanced_color".to_owned(), Value::Object(adv_map));
    }
    if !recipe.skin_tone.is_neutral() {
        map.insert("skin_tone".to_owned(), json!(recipe.skin_tone));
    }
    if !recipe.levels.is_identity() {
        map.insert("levels".to_owned(), json!(recipe.levels));
    }
    if !recipe.bw.is_neutral() {
        map.insert("bw".to_owned(), json!(recipe.bw));
    }
    if !recipe.lens.is_identity() {
        map.insert("lens".to_owned(), json!(recipe.lens));
    }
    if !recipe.geometry.is_identity()
        || recipe.geometry.aspect_preset != default.geometry.aspect_preset
    {
        map.insert("geometry".to_owned(), json!(recipe.geometry));
    }
    if !recipe.layers.is_empty() {
        map.insert("layers".to_owned(), json!(recipe.layers));
    }
    if !recipe.heal_spots.is_empty() {
        map.insert("heal_spots".to_owned(), json!(recipe.heal_spots));
    }
    if recipe.processing_mode != default.processing_mode {
        map.insert("processing_mode".to_owned(), json!(recipe.processing_mode));
    }
    if !recipe.film_negative.is_default() {
        map.insert("film_negative".to_owned(), json!(recipe.film_negative));
    }
    if let Some(name) = &recipe.preset_name
        && name != "Neutral"
    {
        map.insert("preset_name".to_owned(), json!(name));
    }
    map
}

/// Computes human- and machine-readable field-by-field diffs between two `DevelopRecipe`s.
pub fn recipe_field_diffs(before: &DevelopRecipe, after: &DevelopRecipe) -> Vec<FieldDiff> {
    let mut diffs = Vec::new();
    for spec in SCALAR_PARAMS {
        let a = (spec.get)(before);
        let b = (spec.get)(after);
        if (a - b).abs() > 1e-5 {
            diffs.push(FieldDiff {
                field: spec.id.to_owned(),
                before: json!(round4(a)),
                after: json!(round4(b)),
            });
        }
    }
    if before.film_curve != after.film_curve {
        diffs.push(FieldDiff {
            field: "film_curve".to_owned(),
            before: json!(before.film_curve),
            after: json!(after.film_curve),
        });
    }
    if before.wb_preset != after.wb_preset {
        diffs.push(FieldDiff {
            field: "wb_preset".to_owned(),
            before: json!(before.wb_preset),
            after: json!(after.wb_preset),
        });
    }
    if before.clarity_method != after.clarity_method {
        diffs.push(FieldDiff {
            field: "clarity_method".to_owned(),
            before: json!(before.clarity_method),
            after: json!(after.clarity_method),
        });
    }
    if before.grain_type != after.grain_type {
        diffs.push(FieldDiff {
            field: "grain_type".to_owned(),
            before: json!(before.grain_type),
            after: json!(after.grain_type),
        });
    }
    if before.vignette_method != after.vignette_method {
        diffs.push(FieldDiff {
            field: "vignette_method".to_owned(),
            before: json!(before.vignette_method),
            after: json!(after.vignette_method),
        });
    }
    for (name, wa, wb) in [
        ("shadows_wheel", before.shadows_wheel, after.shadows_wheel),
        (
            "midtones_wheel",
            before.midtones_wheel,
            after.midtones_wheel,
        ),
        (
            "highlights_wheel",
            before.highlights_wheel,
            after.highlights_wheel,
        ),
        ("master_wheel", before.master_wheel, after.master_wheel),
    ] {
        if wa != wb {
            diffs.push(FieldDiff {
                field: name.to_owned(),
                before: json!(wa),
                after: json!(wb),
            });
        }
    }
    for (idx, (ha, hb)) in before.hsl.iter().zip(after.hsl.iter()).enumerate() {
        if ha != hb {
            diffs.push(FieldDiff {
                field: format!("hsl.{}", HSL_BAND_NAMES[idx].to_ascii_lowercase()),
                before: json!(ha),
                after: json!(hb),
            });
        }
    }
    for (idx, (sa, sb)) in before
        .advanced_color
        .iter()
        .zip(after.advanced_color.iter())
        .enumerate()
    {
        if sa != sb {
            diffs.push(FieldDiff {
                field: format!("advanced_color.{idx}"),
                before: json!(sa),
                after: json!(sb),
            });
        }
    }
    if before.skin_tone != after.skin_tone {
        diffs.push(FieldDiff {
            field: "skin_tone".to_owned(),
            before: json!(before.skin_tone),
            after: json!(after.skin_tone),
        });
    }
    if before.curves != after.curves {
        diffs.push(FieldDiff {
            field: "curves".to_owned(),
            before: json!(before.curves),
            after: json!(after.curves),
        });
    }
    if before.levels != after.levels {
        diffs.push(FieldDiff {
            field: "levels".to_owned(),
            before: json!(before.levels),
            after: json!(after.levels),
        });
    }
    if before.bw != after.bw {
        diffs.push(FieldDiff {
            field: "bw".to_owned(),
            before: json!(before.bw),
            after: json!(after.bw),
        });
    }
    if before.lens != after.lens {
        diffs.push(FieldDiff {
            field: "lens".to_owned(),
            before: json!(before.lens),
            after: json!(after.lens),
        });
    }
    if before.geometry != after.geometry {
        diffs.push(FieldDiff {
            field: "geometry".to_owned(),
            before: json!(before.geometry),
            after: json!(after.geometry),
        });
    }
    if before.layers != after.layers {
        diffs.push(FieldDiff {
            field: "layers".to_owned(),
            before: json!(before.layers),
            after: json!(after.layers),
        });
    }
    if before.heal_spots != after.heal_spots {
        diffs.push(FieldDiff {
            field: "heal_spots".to_owned(),
            before: json!(before.heal_spots),
            after: json!(after.heal_spots),
        });
    }
    if before.processing_mode != after.processing_mode {
        diffs.push(FieldDiff {
            field: "processing_mode".to_owned(),
            before: json!(before.processing_mode),
            after: json!(after.processing_mode),
        });
    }
    if before.film_negative != after.film_negative {
        diffs.push(FieldDiff {
            field: "film_negative".to_owned(),
            before: json!(before.film_negative),
            after: json!(after.film_negative),
        });
    }
    diffs
}

/// Emits machine-readable schema for `rd schema recipe`.
pub fn recipe_schema_json() -> Value {
    let presets: Vec<Value> = builtin_presets()
        .into_iter()
        .map(|p| json!({ "id": p.id, "name": p.name }))
        .collect();
    json!({
        "schema_version": crate::RECIPE_SCHEMA_VERSION,
        "usage_note": "Use each parameter's exact `id` directly in `set` (e.g. `levels.in_black`, `shadows_wheel.hue_x`, `curves.rgb`, `hsl.orange.saturation`, `advanced_color.0.hue_shift`, `skin_tone.uniformity_hue`, `lens.distortion`, `geometry.keystone_vertical`, `film_negative.density`) — do NOT prepend `group.`. Tone curves accept [[x,y],...], flat [x0,y0,...], keyed maps, \"0,0;0.25,0.2;1,1\", or \"reset\".",
        "parameters": all_param_descriptors(),
        "tool_groups": ["tone", "wb", "color", "wheels", "hsl", "advanced_color", "skin_tone", "levels", "curves", "detail", "bw", "effects", "lens", "geometry", "layers", "heal", "film_negative"],
        "hsl_bands": HSL_BAND_NAMES.iter().map(|s| s.to_ascii_lowercase()).collect::<Vec<_>>(),
        "builtin_presets": presets,
    })
}

/// Parses tone-curve control points from `"reset"`, JSON `[[x,y],...]`, JSON flat `[x0,y0,...]`,
/// JSON keyed map `{"0": 0, "0.5": 0.55, "1": 1}`, or semicolon/comma pairs `"0,0;0.25,0.2;1,1"`.
pub fn parse_curve_points_str(raw: &str, key: &str) -> Result<Vec<[f32; 2]>, CoreError> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("reset") || trimmed.eq_ignore_ascii_case("identity") {
        return Ok(crate::default_curve_points());
    }
    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        let val: Value = serde_json::from_str(trimmed)
            .map_err(|e| CoreError::Validation(format!("Invalid curve JSON for {key}: {e}")))?;
        return parse_curve_points_value(&val, key);
    }

    let mut pts = Vec::new();
    if trimmed.contains(';') {
        for pair in trimmed.split(';') {
            let p = pair.trim();
            if p.is_empty() {
                continue;
            }
            let (xs, ys) = p.split_once(',').ok_or_else(|| {
                CoreError::Validation(format!(
                    "Invalid curve point '{p}' for {key} (expected x,y)"
                ))
            })?;
            pts.push([
                parse_finite_f32(xs.trim(), key)?,
                parse_finite_f32(ys.trim(), key)?,
            ]);
        }
    } else {
        let nums: Result<Vec<f32>, _> = trimmed
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .map(|s| parse_finite_f32(s.trim(), key))
            .collect();
        let nums = nums?;
        if nums.len() < 4 || nums.len() % 2 != 0 {
            return Err(CoreError::Validation(format!(
                "Curve {key} requires at least 2 (x,y) pairs"
            )));
        }
        for pair in nums.as_chunks::<2>().0 {
            pts.push([pair[0], pair[1]]);
        }
    }
    sanitize_curve_points(pts, key)
}

/// Parses tone-curve control points from any intuitive JSON representation.
pub fn parse_curve_points_value(val: &Value, key: &str) -> Result<Vec<[f32; 2]>, CoreError> {
    match val {
        Value::String(s) => parse_curve_points_str(s, key),
        Value::Array(arr) => {
            if arr.is_empty() {
                return Err(CoreError::Validation(format!(
                    "Curve {key} requires at least 2 control points"
                )));
            }
            if arr[0].is_array() {
                let mut pts = Vec::with_capacity(arr.len());
                for item in arr {
                    let pair = item.as_array().ok_or_else(|| {
                        CoreError::Validation(format!("Curve {key} expects [x,y] pairs"))
                    })?;
                    if pair.len() != 2 {
                        return Err(CoreError::Validation(format!(
                            "Curve {key} point must have 2 elements [x,y]"
                        )));
                    }
                    let x = pair[0].as_f64().ok_or_else(|| {
                        CoreError::Validation(format!("Curve {key} x must be a number"))
                    })? as f32;
                    let y = pair[1].as_f64().ok_or_else(|| {
                        CoreError::Validation(format!("Curve {key} y must be a number"))
                    })? as f32;
                    pts.push([x, y]);
                }
                sanitize_curve_points(pts, key)
            } else if arr[0].is_number() {
                if arr.len() < 4 || arr.len() % 2 != 0 {
                    return Err(CoreError::Validation(format!(
                        "Flat curve array for {key} needs an even count >= 4 numbers"
                    )));
                }
                let mut pts = Vec::with_capacity(arr.len() / 2);
                for pair in arr.as_chunks::<2>().0 {
                    let x = pair[0].as_f64().ok_or_else(|| {
                        CoreError::Validation(format!("Curve {key} value must be a number"))
                    })? as f32;
                    let y = pair[1].as_f64().ok_or_else(|| {
                        CoreError::Validation(format!("Curve {key} value must be a number"))
                    })? as f32;
                    pts.push([x, y]);
                }
                sanitize_curve_points(pts, key)
            } else {
                Err(CoreError::Validation(format!(
                    "Unsupported array element type in curve {key}"
                )))
            }
        }
        Value::Object(map) => {
            if let Some(inner) = map.get("set") {
                return parse_curve_points_value(inner, key);
            }
            let mut pts = Vec::with_capacity(map.len());
            for (k, v) in map {
                let x = parse_finite_f32(k.trim(), key)?;
                let y = v.as_f64().ok_or_else(|| {
                    CoreError::Validation(format!("Curve {key} map values must be numbers"))
                })? as f32;
                pts.push([x, y]);
            }
            sanitize_curve_points(pts, key)
        }
        _ => Err(CoreError::Validation(format!(
            "Unsupported curve value for {key}; pass [[x,y],...], flat [x0,y0,...], keyed map, or \"reset\""
        ))),
    }
}

fn sanitize_curve_points(pts: Vec<[f32; 2]>, key: &str) -> Result<Vec<[f32; 2]>, CoreError> {
    if pts.len() < 2 {
        return Err(CoreError::Validation(format!(
            "Curve {key} requires at least 2 control points"
        )));
    }
    let mut clean: Vec<[f32; 2]> = pts
        .into_iter()
        .map(|[x, y]| {
            if !x.is_finite() || !y.is_finite() {
                return Err(CoreError::Validation(format!(
                    "Curve {key} contains non-finite coordinate"
                )));
            }
            Ok([x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)])
        })
        .collect::<Result<Vec<_>, _>>()?;
    clean.sort_by(|a, b| a[0].total_cmp(&b[0]));
    Ok(clean)
}

fn parse_crop_rect_str(raw: &str, key: &str) -> Result<[f32; 4], CoreError> {
    let trimmed = raw.trim();
    if trimmed.starts_with('[') {
        let arr: [f32; 4] = serde_json::from_str(trimmed)
            .map_err(|e| CoreError::Validation(format!("Invalid crop rect for {key}: {e}")))?;
        return Ok(arr);
    }
    let nums: Result<Vec<f32>, _> = trimmed
        .split(',')
        .map(|s| parse_finite_f32(s.trim(), key))
        .collect();
    let nums = nums?;
    if nums.len() != 4 {
        return Err(CoreError::Validation(format!(
            "{key} expects 4 numbers [x,y,w,h] (got {})",
            nums.len()
        )));
    }
    Ok([nums[0], nums[1], nums[2], nums[3]])
}

fn wheel_get(w: &crate::ColorWheel, comp: &str, key: &str) -> Result<f32, CoreError> {
    match comp {
        "hue_x" | "x" => Ok(w.hue_x),
        "hue_y" | "y" => Ok(w.hue_y),
        "luma" | "l" => Ok(w.luma),
        _ => Err(CoreError::UnknownParam(key.to_owned())),
    }
}

fn wheel_set(
    w: &mut crate::ColorWheel,
    comp: &str,
    v: f32,
    delta: bool,
    key: &str,
) -> Result<(), CoreError> {
    let slot = match comp {
        "hue_x" | "x" => &mut w.hue_x,
        "hue_y" | "y" => &mut w.hue_y,
        "luma" | "l" => &mut w.luma,
        _ => return Err(CoreError::UnknownParam(key.to_owned())),
    };
    let target = if delta { *slot + v } else { v };
    if !delta && !(-1.0..=1.0).contains(&target) {
        return Err(CoreError::Validation(format!(
            "{key} must be within -1.0..=1.0 (got {target})"
        )));
    }
    *slot = target.clamp(-1.0, 1.0);
    Ok(())
}

fn parse_slice_index(raw: &str) -> Option<usize> {
    let idx: usize = raw.trim().parse().ok()?;
    (idx < ADVANCED_COLOR_SLICE_COUNT).then_some(idx)
}

fn set_clamped_f32(
    slot: &mut f32,
    v: f32,
    delta: bool,
    min: f32,
    max: f32,
    key: &str,
) -> Result<(), CoreError> {
    let target = if delta { *slot + v } else { v };
    if !delta && (target < min || target > max) {
        return Err(CoreError::Validation(format!(
            "{key} must be within {min}..={max} (got {target})"
        )));
    }
    *slot = target.clamp(min, max);
    Ok(())
}

fn parse_finite_f32(raw: &str, key: &str) -> Result<f32, CoreError> {
    let v: f32 = raw
        .parse()
        .map_err(|_| CoreError::Validation(format!("Invalid number for {key}: '{raw}'")))?;
    if !v.is_finite() {
        return Err(CoreError::Validation(format!(
            "Non-finite number for {key}: '{raw}'"
        )));
    }
    Ok(v)
}

fn parse_bool_str(raw: &str, key: &str) -> Result<bool, CoreError> {
    match raw.to_ascii_lowercase().as_str() {
        "true" | "1" | "on" | "yes" => Ok(true),
        "false" | "0" | "off" | "no" => Ok(false),
        _ => Err(CoreError::Validation(format!(
            "Invalid boolean for {key}: '{raw}' (expected true|false|on|off)"
        ))),
    }
}

#[inline]
fn round4(v: f32) -> f32 {
    (v * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Self-enforcing GUI ↔ CLI parity guard: fails CI if a new field is added to
    /// `DevelopRecipe` without registering it in `registry.rs`.
    #[test]
    fn registry_covers_all_recipe_fields() {
        let val = serde_json::to_value(DevelopRecipe::default()).expect("serialize default recipe");
        let obj = val.as_object().expect("recipe must be JSON object");

        let mut covered: HashSet<String> = SCALAR_PARAMS.iter().map(|s| s.id.to_owned()).collect();
        for structural in [
            "schema_version",
            "preset_name",
            "film_curve",
            "wb_preset",
            "clarity_method",
            "grain_type",
            "vignette_method",
            "shadows_wheel",
            "midtones_wheel",
            "highlights_wheel",
            "master_wheel",
            "curves",
            "hsl",
            "advanced_color",
            "skin_tone",
            "levels",
            "bw",
            "lens",
            "geometry",
            "layers",
            "heal_spots",
            "processing_mode",
            "film_negative",
        ] {
            covered.insert(structural.to_owned());
        }

        for key in obj.keys() {
            assert!(
                covered.contains(key),
                "DevelopRecipe field '{key}' is not registered in crates/core/src/registry.rs! Register it to keep GUI and CLI in 1:1 parity."
            );
        }
    }

    #[test]
    fn set_and_nudge_param_by_dot_path() {
        let mut r = DevelopRecipe::default();
        set_param_by_str(&mut r, "exposure", "0.5", false, None).unwrap();
        set_param_by_str(&mut r, "exposure", "0.25", true, None).unwrap();
        assert!((r.exposure - 0.75).abs() < 1e-6);

        set_param_by_str(&mut r, "hsl.orange.saturation", "-0.2", false, None).unwrap();
        assert!((r.hsl[1].saturation + 0.2).abs() < 1e-6);

        let err = set_param_by_str(&mut r, "exposure", "9.0", false, None);
        assert!(err.is_err());
    }

    #[test]
    fn tone_group_reset_and_copy_symmetry() {
        let src = DevelopRecipe {
            exposure: 0.65,
            contrast: 0.25,
            brightness: 0.15,
            highlights: -0.4,
            shadows: 0.3,
            whites: 0.1,
            blacks: -0.1,
            levels: crate::LevelsParams {
                in_black: 0.05,
                in_mid: 0.12,
                in_white: 0.92,
                out_black: 0.02,
                out_white: 0.98,
                ..crate::LevelsParams::default()
            },
            temperature: 0.4,
            ..DevelopRecipe::default()
        };

        let mut dst = DevelopRecipe::default();
        copy_recipe_groups(&mut dst, &src, &["tone".to_owned()]).expect("copy tone group");
        assert_eq!(dst.exposure, src.exposure);
        assert_eq!(dst.levels, src.levels);
        assert_eq!(dst.temperature, 0.0);

        reset_recipe_group(&mut dst, "tone").expect("reset tone group");
        assert!(dst.is_neutral());
        assert_eq!(dst.levels, crate::LevelsParams::default());
    }

    #[test]
    fn phase1_dot_paths_and_group_symmetry() {
        let mut r = DevelopRecipe::default();
        set_param_by_str(&mut r, "film_curve", "film_extra_shadow", false, None).unwrap();
        assert_eq!(r.film_curve, FilmCurve::FilmExtraShadow);

        set_param_by_str(&mut r, "wb_preset", "tungsten", false, None).unwrap();
        assert_eq!(r.wb_preset, WbPreset::Tungsten);
        assert!(r.temperature < -0.2);

        set_param_by_str(&mut r, "curves.rgb", "0,0;0.5,0.6;1,1", false, None).unwrap();
        assert!(!r.curves.is_identity());
        assert_eq!(r.curves.rgb.len(), 3);

        set_param_by_str(&mut r, "levels.red.in_black", "0.08", false, None).unwrap();
        set_param_by_str(&mut r, "levels.green.in_mid", "0.25", false, None).unwrap();
        set_param_by_str(&mut r, "levels.blue.out_white", "0.92", false, None).unwrap();
        assert!((get_scalar_by_path(&r, "levels.red.in_black").unwrap() - 0.08).abs() < 1e-6);
        assert!((get_scalar_by_path(&r, "levels.green.in_mid").unwrap() - 0.25).abs() < 1e-6);
        assert!((get_scalar_by_path(&r, "levels.blue.out_white").unwrap() - 0.92).abs() < 1e-6);
        assert!(set_param_by_str(&mut r, "levels.in_black", "0.97", false, None).is_err());
        assert!(set_param_by_str(&mut r, "levels.red.in_white", "0.03", false, None).is_err());

        set_param_by_str(&mut r, "wheels.master_wheel.hue_x", "0.3", false, None).unwrap();
        set_param_by_str(&mut r, "wheels.master.luma", "-0.15", false, None).unwrap();
        assert!((get_scalar_by_path(&r, "master_wheel.hue_x").unwrap() - 0.3).abs() < 1e-6);
        assert!((get_scalar_by_path(&r, "master_wheel.luma").unwrap() + 0.15).abs() < 1e-6);

        set_param_by_str(&mut r, "bw.enabled", "true", false, None).unwrap();
        set_param_by_str(&mut r, "bw.split_shadow_hue", "210", false, None).unwrap();
        set_param_by_str(&mut r, "bw.split_shadow_sat", "0.4", false, None).unwrap();
        set_param_by_str(&mut r, "bw.split_highlight_hue", "42", false, None).unwrap();
        set_param_by_str(&mut r, "bw.split_highlight_sat", "0.3", false, None).unwrap();
        assert!((get_scalar_by_path(&r, "bw.split_shadow_hue").unwrap() - 210.0).abs() < 1e-5);

        set_param_by_str(&mut r, "clarity_method", "punch", false, None).unwrap();
        set_param_by_str(&mut r, "sharpen_radius", "2.2", false, None).unwrap();
        set_param_by_str(&mut r, "sharpen_threshold", "0.2", false, None).unwrap();
        set_param_by_str(&mut r, "sharpen_halo", "0.5", false, None).unwrap();
        set_param_by_str(&mut r, "nr_color", "0.6", false, None).unwrap();
        set_param_by_str(&mut r, "grain_type", "silver_rich", false, None).unwrap();
        set_param_by_str(&mut r, "grain_size", "0.75", false, None).unwrap();
        assert_eq!(r.clarity_method, ClarityMethod::Punch);
        assert_eq!(r.grain_type, GrainType::SilverRich);

        // Verify copy + reset symmetry across all groups touched above.
        for grp in [
            "tone", "wb", "wheels", "curves", "levels", "bw", "detail", "effects",
        ] {
            let mut dst = DevelopRecipe::default();
            copy_recipe_groups(&mut dst, &r, &[grp.to_owned()]).unwrap();
            reset_recipe_group(&mut dst, grp).unwrap();
            assert!(dst.is_neutral(), "group '{grp}' failed copy/reset symmetry");
        }
    }

    #[test]
    fn phase2_dot_paths_and_group_symmetry() {
        let mut r = DevelopRecipe::default();
        set_param_by_str(&mut r, "vignette_method", "circular_on_crop", false, None).unwrap();
        assert_eq!(r.vignette_method, VignetteMethod::CircularOnCrop);

        set_param_by_str(&mut r, "advanced_color.0.enabled", "true", false, None).unwrap();
        set_param_by_str(&mut r, "advanced_color.0.target_hue", "210", false, None).unwrap();
        set_param_by_str(&mut r, "advanced_color.0.hue_range", "45", false, None).unwrap();
        set_param_by_str(&mut r, "advanced_color.0.uniformity", "0.65", false, None).unwrap();
        assert!(
            (get_scalar_by_path(&r, "advanced_color.0.uniformity").unwrap() - 0.65).abs() < 1e-6
        );
        assert!(set_param_by_str(&mut r, "advanced_color.4.enabled", "true", false, None).is_err());

        set_param_by_str(&mut r, "skin_tone.enabled", "true", false, None).unwrap();
        set_param_by_str(&mut r, "skin_tone.uniformity_hue", "0.7", false, None).unwrap();
        set_param_by_str(&mut r, "skin_tone.amount_lightness", "0.2", false, None).unwrap();
        assert!((get_scalar_by_path(&r, "skin_tone.uniformity_hue").unwrap() - 0.7).abs() < 1e-6);

        set_param_by_str(&mut r, "lens.distortion", "-0.35", false, None).unwrap();
        set_param_by_str(&mut r, "lens.chromatic_aberration", "0.4", false, None).unwrap();
        set_param_by_str(&mut r, "lens.purple_fringing", "0.6", false, None).unwrap();
        set_param_by_str(&mut r, "lens.light_falloff", "0.5", false, None).unwrap();
        assert!((get_scalar_by_path(&r, "lens.distortion").unwrap() + 0.35).abs() < 1e-6);

        set_param_by_str(&mut r, "geometry.keystone_vertical", "0.4", false, None).unwrap();
        set_param_by_str(&mut r, "geometry.keystone_horizontal", "-0.2", false, None).unwrap();
        set_param_by_str(&mut r, "geometry.keystone_aspect", "0.25", false, None).unwrap();
        set_param_by_str(&mut r, "geometry.keystone_amount", "0.9", false, None).unwrap();
        assert!((get_scalar_by_path(&r, "geometry.keystone_vertical").unwrap() - 0.4).abs() < 1e-6);

        for grp in [
            "color",
            "advanced_color",
            "skin_tone",
            "lens",
            "geometry",
            "effects",
        ] {
            let mut dst = DevelopRecipe::default();
            copy_recipe_groups(&mut dst, &r, &[grp.to_owned()]).unwrap();
            reset_recipe_group(&mut dst, grp).unwrap();
            assert!(
                dst.is_neutral(),
                "Phase 2 group '{grp}' failed copy/reset symmetry"
            );
        }
    }
}
