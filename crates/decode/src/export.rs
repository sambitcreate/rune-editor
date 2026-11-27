//! Full-resolution and web-sized multi-core photo export (JPEG, PNG8, PNG16, TIFF16, WebP).
//!
//! Evaluates scene-referred linear `Rgba16Float` pixels through optional geometry/crop,
//! high-quality linear-light Catmull-Rom downscaling, `rune_editor_core::CompiledRecipe`
//! plus optical vignette and subtle film grain matching `shaders/develop.wgsl`, filmic
//! tonemapping, optional screen output sharpening, and sRGB 8-bit/16-bit encoding.

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static EXPORT_TMP_SEQ: AtomicU64 = AtomicU64::new(1);

use half::f16;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, ImageBuffer, ImageFormat, Rgb};
use rayon::prelude::*;
use rune_editor_core::{
    AdjustmentLayer, ClarityMethod, CompiledRecipe, DevelopRecipe, GrainType, HealMode, HealSpot,
    LayerMaskAtlas, MAX_ADJUSTMENT_LAYERS, MAX_HEAL_SPOTS, VignetteMethod,
    apply_lens_distortion_uv, apply_local_adjustments_linear, bake_layer_mask_atlas,
    filmic_tonemap_with_curve, linear_to_srgb,
};
use serde::{Deserialize, Serialize};

use crate::raw::fit_dimensions;
use crate::{DecodeError, LinearImage};

const fn default_true() -> bool {
    true
}

const fn default_watermark_opacity() -> f32 {
    0.45
}

const fn default_watermark_scale() -> f32 {
    1.0
}

fn default_naming_template() -> String {
    "{ImageName}_{Recipe}".to_owned()
}

/// Output container and bit-depth for photo export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// 8-bit JPEG (`quality` clamped to `1..=100`).
    Jpeg { quality: u8 },
    /// 16-bit lossless PNG.
    Png16,
    /// 8-bit lossless PNG.
    Png8,
    /// 16-bit TIFF.
    Tiff16,
    /// 8-bit lossless WebP.
    WebP,
}

impl Default for ExportFormat {
    fn default() -> Self {
        Self::Jpeg { quality: 92 }
    }
}

impl ExportFormat {
    /// Canonical lowercase file extension for this container format.
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Jpeg { .. } => "jpg",
            Self::Png16 | Self::Png8 => "png",
            Self::Tiff16 => "tif",
            Self::WebP => "webp",
        }
    }
}

/// Output color space for exported raster files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportColorSpace {
    #[default]
    Srgb,
    DisplayP3,
}

impl ExportColorSpace {
    pub const ALL: [Self; 2] = [Self::Srgb, Self::DisplayP3];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Srgb => "sRGB",
            Self::DisplayP3 => "Display P3",
        }
    }

    pub const fn cli_slug(self) -> &'static str {
        match self {
            Self::Srgb => "srgb",
            Self::DisplayP3 => "display-p3",
        }
    }

    pub fn from_cli_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "srgb" => Some(Self::Srgb),
            "display-p3" | "display_p3" | "p3" => Some(Self::DisplayP3),
            _ => None,
        }
    }
}

/// Converts linear Rec.709 / sRGB (D65) into linear Display P3 (D65) using the exact
/// CIE 1931 chromaticity matrix (row sums equal `1.0` for exact neutral gray invariance).
#[inline]
pub fn linear_srgb_to_display_p3(rgb: [f32; 3]) -> [f32; 3] {
    let r = rgb[0].max(0.0);
    let g = rgb[1].max(0.0);
    let b = rgb[2].max(0.0);
    [
        (0.822_462_f32 * r + 0.177_538_f32 * g).max(0.0),
        (0.033_194_2_f32 * r + 0.966_805_8_f32 * g).max(0.0),
        (0.017_082_6_f32 * r + 0.072_397_4_f32 * g + 0.910_520_f32 * b).max(0.0),
    ]
}

/// Anchor corner or center placement for an exported text watermark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatermarkPosition {
    #[default]
    BottomRight,
    BottomLeft,
    TopRight,
    TopLeft,
    Center,
}

impl WatermarkPosition {
    pub const ALL: [Self; 5] = [
        Self::BottomRight,
        Self::BottomLeft,
        Self::TopRight,
        Self::TopLeft,
        Self::Center,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::BottomRight => "Bottom Right",
            Self::BottomLeft => "Bottom Left",
            Self::TopRight => "Top Right",
            Self::TopLeft => "Top Left",
            Self::Center => "Center",
        }
    }

    pub const fn cli_slug(self) -> &'static str {
        match self {
            Self::BottomRight => "bottom_right",
            Self::BottomLeft => "bottom_left",
            Self::TopRight => "top_right",
            Self::TopLeft => "top_left",
            Self::Center => "center",
        }
    }

    pub fn from_cli_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "bottom_right" | "br" => Some(Self::BottomRight),
            "bottom_left" | "bl" => Some(Self::BottomLeft),
            "top_right" | "tr" => Some(Self::TopRight),
            "top_left" | "tl" => Some(Self::TopLeft),
            "center" | "middle" => Some(Self::Center),
            _ => None,
        }
    }
}

/// Anti-aliased text watermark rendered onto exported photographs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextWatermark {
    pub enabled: bool,
    pub text: String,
    /// Opacity in `0.0..=1.0` (default `0.45`).
    #[serde(default = "default_watermark_opacity")]
    pub opacity: f32,
    /// Relative text scale multiplier in `0.5..=3.0` (default `1.0`).
    #[serde(default = "default_watermark_scale")]
    pub scale: f32,
    pub position: WatermarkPosition,
}

impl Default for TextWatermark {
    fn default() -> Self {
        Self {
            enabled: false,
            text: String::new(),
            opacity: default_watermark_opacity(),
            scale: default_watermark_scale(),
            position: WatermarkPosition::BottomRight,
        }
    }
}

impl TextWatermark {
    pub fn is_active(&self) -> bool {
        self.enabled
            && !self.text.trim().is_empty()
            && self.opacity.is_finite()
            && self.opacity > 1e-4
    }

    pub fn sanitized(&self) -> Self {
        Self {
            enabled: self.enabled,
            text: self.text.trim().chars().take(128).collect(),
            opacity: if self.opacity.is_finite() {
                self.opacity.clamp(0.0, 1.0)
            } else {
                default_watermark_opacity()
            },
            scale: if self.scale.is_finite() {
                self.scale.clamp(0.5, 3.0)
            } else {
                default_watermark_scale()
            },
            position: self.position,
        }
    }
}

/// Configuration options for [`export_image_with_options`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportOptions {
    pub format: ExportFormat,
    /// Optional longest-edge cap in pixels (e.g. `Some(2048)` for web export).
    pub max_edge: Option<u32>,
    /// When `true`, applies `recipe.geometry` (rotate, flip, straighten, crop, keystone).
    #[serde(default = "default_true")]
    pub respect_crop: bool,
    /// Post-resize screen output sharpening strength in `0.0..=1.0`.
    pub output_sharpen: f32,
    /// Strips EXIF / camera metadata from exported files when `true`.
    #[serde(default)]
    pub strip_metadata: bool,
    /// Optional anti-aliased text watermark composited onto the exported image.
    #[serde(default)]
    pub watermark: TextWatermark,
    /// Output color space (`Srgb` default, or `DisplayP3`).
    #[serde(default)]
    pub color_space: ExportColorSpace,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            format: ExportFormat::Jpeg { quality: 92 },
            max_edge: None,
            respect_crop: true,
            output_sharpen: 0.0,
            strip_metadata: false,
            watermark: TextWatermark::default(),
            color_space: ExportColorSpace::Srgb,
        }
    }
}

/// Metadata context used to expand Capture One-style dynamic naming templates.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NamingContext {
    pub image_name: String,
    pub counter: usize,
    pub date: String,
    pub camera: String,
    pub lens: String,
    pub iso: String,
    pub variant_index: usize,
    pub recipe_name: String,
}

/// Sanitizes an expanded filename stem so it can never introduce path separators, `..` traversal,
/// hidden leading dots, or control characters.
pub fn sanitize_filename_stem(raw: &str) -> String {
    let mut cleaned = String::with_capacity(raw.len().min(160));
    for ch in raw.chars() {
        if ch.is_control()
            || matches!(
                ch,
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0'
            )
        {
            cleaned.push('_');
        } else {
            cleaned.push(ch);
        }
    }
    while cleaned.contains("..") {
        cleaned = cleaned.replace("..", "_");
    }
    // Collapse consecutive underscores and spaces cleanly.
    let mut compact = String::with_capacity(cleaned.len().min(160));
    let mut prev_sep = false;
    for ch in cleaned.chars() {
        if ch == '_' || ch.is_whitespace() {
            if !prev_sep && !compact.is_empty() {
                compact.push('_');
                prev_sep = true;
            }
        } else {
            compact.push(ch);
            prev_sep = false;
        }
        if compact.len() >= 160 {
            break;
        }
    }
    let trimmed = compact.trim_matches(|c: char| c == '.' || c == '_' || c.is_whitespace());
    if trimmed.is_empty() {
        "export".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Expands Capture One-style dynamic naming tokens (`{ImageName}`, `{Counter}`, `{Date}`,
/// `{Camera}`, `{Lens}`, `{ISO}`, `{Variant}`, `{Recipe}`) and sanitizes the resulting stem
/// against path traversal, separators, and control characters.
pub fn expand_naming_template(template: &str, ctx: &NamingContext) -> String {
    let tpl = if template.trim().is_empty() {
        "{ImageName}"
    } else {
        template.trim()
    };

    let image_name = if ctx.image_name.trim().is_empty() {
        "image".to_owned()
    } else {
        ctx.image_name.trim().to_owned()
    };
    let counter = format!("{:04}", ctx.counter);
    let date = if ctx.date.trim().is_empty() {
        "undated".to_owned()
    } else {
        ctx.date.trim().to_owned()
    };
    let camera = if ctx.camera.trim().is_empty() {
        "camera".to_owned()
    } else {
        ctx.camera.trim().to_owned()
    };
    let lens = if ctx.lens.trim().is_empty() {
        "lens".to_owned()
    } else {
        ctx.lens.trim().to_owned()
    };
    let iso = if ctx.iso.trim().is_empty() {
        "ISO0".to_owned()
    } else {
        ctx.iso.trim().to_owned()
    };
    let variant = format!("V{}", ctx.variant_index + 1);
    let recipe = if ctx.recipe_name.trim().is_empty() {
        "export".to_owned()
    } else {
        ctx.recipe_name.trim().to_owned()
    };

    let mut out = String::with_capacity(tpl.len() + 32);
    let bytes = tpl.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{'
            && let Some(end_rel) = tpl[i + 1..].find('}')
        {
            let token = &tpl[i + 1..i + 1 + end_rel];
            let replacement = match token.trim().to_ascii_lowercase().as_str() {
                "imagename" | "image_name" => Some(image_name.as_str()),
                "counter" => Some(counter.as_str()),
                "date" => Some(date.as_str()),
                "camera" => Some(camera.as_str()),
                "lens" => Some(lens.as_str()),
                "iso" => Some(iso.as_str()),
                "variant" => Some(variant.as_str()),
                "recipe" | "recipe_name" => Some(recipe.as_str()),
                _ => None,
            };
            if let Some(val) = replacement {
                out.push_str(val);
            } else {
                out.push_str(token);
            }
            i += end_rel + 2;
            continue;
        }
        let ch = tpl[i..].chars().next().unwrap_or('_');
        out.push(ch);
        i += ch.len_utf8();
    }

    sanitize_filename_stem(&out)
}

/// Capture One-style Process Recipe specification for simultaneous Multi-Recipe Export.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportRecipeSpec {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub subfolder: String,
    #[serde(default = "default_naming_template")]
    pub naming_template: String,
    #[serde(default)]
    pub options: ExportOptions,
}

impl Default for ExportRecipeSpec {
    fn default() -> Self {
        Self {
            name: "Full-Res JPEG".to_owned(),
            enabled: true,
            subfolder: String::new(),
            naming_template: default_naming_template(),
            options: ExportOptions::default(),
        }
    }
}

/// Result metadata for a single output written by [`export_multi_recipe`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiRecipeExportItem {
    pub recipe_name: String,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
}

/// Returns the 3 default Capture One-style export process recipes:
/// `Full-Res JPEG`, `Web 2048px`, and `Archival 16-bit TIFF`.
pub fn builtin_export_recipes() -> Vec<ExportRecipeSpec> {
    vec![
        ExportRecipeSpec {
            name: "Full-Res JPEG".to_owned(),
            enabled: true,
            subfolder: String::new(),
            naming_template: "{ImageName}_{Recipe}".to_owned(),
            options: ExportOptions {
                format: ExportFormat::Jpeg { quality: 92 },
                max_edge: None,
                respect_crop: true,
                output_sharpen: 0.0,
                strip_metadata: false,
                watermark: TextWatermark::default(),
                color_space: ExportColorSpace::Srgb,
            },
        },
        ExportRecipeSpec {
            name: "Web 2048px".to_owned(),
            enabled: true,
            subfolder: "web".to_owned(),
            naming_template: "{ImageName}_{Recipe}".to_owned(),
            options: ExportOptions {
                format: ExportFormat::Jpeg { quality: 86 },
                max_edge: Some(2048),
                respect_crop: true,
                output_sharpen: 0.25,
                strip_metadata: false,
                watermark: TextWatermark::default(),
                color_space: ExportColorSpace::Srgb,
            },
        },
        ExportRecipeSpec {
            name: "Archival 16-bit TIFF".to_owned(),
            enabled: false,
            subfolder: "archival".to_owned(),
            naming_template: "{ImageName}_{Recipe}".to_owned(),
            options: ExportOptions {
                format: ExportFormat::Tiff16,
                max_edge: None,
                respect_crop: true,
                output_sharpen: 0.0,
                strip_metadata: false,
                watermark: TextWatermark::default(),
                color_space: ExportColorSpace::DisplayP3,
            },
        },
    ]
}

fn sanitize_subfolder_rel(raw: &str) -> Result<PathBuf, DecodeError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "." {
        return Ok(PathBuf::new());
    }
    if trimmed.contains('\0') {
        return Err(DecodeError::InvalidPath(
            "Export subfolder contains NUL byte".to_owned(),
        ));
    }
    let p = Path::new(trimmed);
    let mut clean = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::Normal(seg) => {
                let s = seg.to_string_lossy();
                if s.starts_with('.') {
                    return Err(DecodeError::InvalidPath(format!(
                        "Hidden subfolder segment '{s}' is forbidden"
                    )));
                }
                clean.push(seg);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(DecodeError::InvalidPath(format!(
                    "Subfolder '{trimmed}' must be a relative path without '..' or root prefix"
                )));
            }
        }
    }
    Ok(clean)
}

/// Pre-flight planner for [`export_multi_recipe`]: resolves every enabled recipe's destination path,
/// validates subfolder traversal safety, enforces [`guard_not_source`], rejects duplicate output paths,
/// and enforces `overwrite: false` BEFORE writing any file to disk.
pub fn plan_multi_recipe_paths(
    out_dir: &Path,
    recipes: &[ExportRecipeSpec],
    base_ctx: &NamingContext,
    source_path: Option<&Path>,
    overwrite: bool,
) -> Result<Vec<(usize, PathBuf)>, DecodeError> {
    let mut planned = Vec::new();
    let mut seen_canon = HashSet::new();

    for (idx, spec) in recipes.iter().enumerate() {
        if !spec.enabled {
            continue;
        }
        let sub = sanitize_subfolder_rel(&spec.subfolder)?;
        let mut ctx = base_ctx.clone();
        ctx.recipe_name = spec.name.clone();
        let stem = expand_naming_template(&spec.naming_template, &ctx);
        let filename = format!("{stem}.{}", spec.options.format.extension());
        let dest = if sub.as_os_str().is_empty() {
            out_dir.join(filename)
        } else {
            out_dir.join(sub).join(filename)
        };

        if let Some(src) = source_path {
            guard_not_source(src, &dest)?;
        }

        let canon = canonicalize_best_effort(&dest);
        let dedup_key = if cfg!(any(target_os = "macos", target_os = "windows")) {
            canon.to_string_lossy().to_lowercase()
        } else {
            canon.to_string_lossy().into_owned()
        };
        if !seen_canon.insert(dedup_key) {
            return Err(DecodeError::DuplicateDestination(dest));
        }

        if !overwrite && (dest.exists() || dest.symlink_metadata().is_ok()) {
            return Err(DecodeError::DestinationExists(dest));
        }

        planned.push((idx, dest));
    }

    if planned.is_empty() {
        return Err(DecodeError::InvalidPath(
            "No enabled export recipes provided".to_owned(),
        ));
    }
    Ok(planned)
}

/// Executes simultaneous Multi-Recipe Export with atomic pre-flight collision, `guard_not_source`,
/// and `overwrite` checks, writing each enabled recipe atomically via temp-dotfile + `sync_all` + `rename`.
pub fn export_multi_recipe(
    image: &LinearImage,
    develop: &DevelopRecipe,
    out_dir: &Path,
    recipes: &[ExportRecipeSpec],
    base_ctx: &NamingContext,
    source_path: Option<&Path>,
    overwrite: bool,
) -> Result<Vec<MultiRecipeExportItem>, DecodeError> {
    validate_linear_image(image)?;
    let effective_src = source_path.or(image.source_path.as_deref());
    let planned = plan_multi_recipe_paths(out_dir, recipes, base_ctx, effective_src, overwrite)?;

    let mut written = Vec::with_capacity(planned.len());
    for (idx, dest) in planned {
        let spec = &recipes[idx];
        let (out_w, out_h) = export_image_with_options_dims(image, develop, &dest, &spec.options)?;
        written.push(MultiRecipeExportItem {
            recipe_name: spec.name.clone(),
            path: dest,
            width: out_w,
            height: out_h,
        });
    }
    Ok(written)
}

/// Rejects any export destination that resolves to the same filesystem path as `src`,
/// even when `dest` does not exist yet or traverses symlinks / relative `..` segments.
pub fn guard_not_source(src: &Path, dest: &Path) -> Result<(), DecodeError> {
    let c_src = canonicalize_best_effort(src);
    let c_dest = canonicalize_best_effort(dest);
    if c_src == c_dest {
        return Err(DecodeError::SourceOverwrite(src.to_path_buf()));
    }
    Ok(())
}

fn canonicalize_best_effort(path: &Path) -> PathBuf {
    if let Ok(canon) = std::fs::canonicalize(path) {
        return canon;
    }

    // Resolve against CWD first if relative so empty parent (`Path::new("photo.jpg").parent() == ""`)
    // resolves cleanly to the current working directory.
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    };

    // Walk up ancestors until we find the deepest existing directory/file to canonicalize,
    // preserving non-existent tail components (including `..` segments where `file_name()` is `None`).
    let mut tail = Vec::new();
    let mut cur = abs.as_path();
    let mut base: Option<PathBuf> = None;
    while let Some(last) = cur.components().next_back() {
        let Some(parent) = cur.parent() else {
            break;
        };
        if matches!(last, Component::RootDir | Component::Prefix(_)) {
            break;
        }
        tail.push(last.as_os_str().to_os_string());
        if parent.as_os_str().is_empty() {
            if let Ok(c) = std::fs::canonicalize(".") {
                base = Some(c);
            }
            break;
        }
        if let Ok(c) = std::fs::canonicalize(parent) {
            base = Some(c);
            break;
        }
        cur = parent;
    }

    let mut resolved = base.unwrap_or_else(|| cur.to_path_buf());
    for comp in tail.into_iter().rev() {
        resolved.push(comp);
    }

    // Normalize lexical `.` and `..` components on the resulting path, then re-canonicalize
    // if the normalized path now points to an existing entry (e.g. `nonexistent/../existing`).
    let mut normalized = PathBuf::new();
    for comp in resolved.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    std::fs::canonicalize(&normalized).unwrap_or(normalized)
}

fn validate_linear_image(image: &LinearImage) -> Result<(), DecodeError> {
    let width = image.width;
    let height = image.height;
    if width == 0 || height == 0 {
        return Err(DecodeError::EmptyDimensions(width, height));
    }

    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| DecodeError::InvalidBuffer("Export image dimensions overflow".to_owned()))?;
    if image.rgba16f.len() < expected_len {
        return Err(DecodeError::InvalidBuffer(format!(
            "LinearImage buffer too small: expected at least {expected_len} f16 samples, got {}",
            image.rgba16f.len()
        )));
    }
    Ok(())
}

/// Samples a single raw pre-grade linear RGB pixel from `image` at normalized cropped/oriented
/// coordinates `(u, v)` in `[0.0, 1.0]^2`, respecting `recipe.geometry` (including keystone)
/// and `recipe.lens` (distortion + chromatic aberration), WITHOUT running Stages 0.5..=8.
///
/// Use this for raw Film Base (`--pick-film-base`) and raw White Balance eyedroppers.
pub fn sample_raw_linear_rgb_at_uv(
    image: &LinearImage,
    u: f32,
    v: f32,
    recipe: &DevelopRecipe,
) -> [f32; 3] {
    if image.width == 0 || image.height == 0 || image.rgba16f.len() < 4 {
        return [0.0, 0.0, 0.0];
    }
    let cu = if u.is_finite() {
        u.clamp(0.0, 1.0)
    } else {
        0.5
    };
    let cv = if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.5
    };
    let [su, sv] =
        recipe
            .geometry
            .cropped_to_source_uv_unclamped(cu, cv, image.width, image.height);
    if !su.is_finite() || !sv.is_finite() {
        return [0.0, 0.0, 0.0];
    }
    let lens = recipe.lens.sanitized();
    let [lu, lv] = apply_lens_distortion_uv([su, sv], lens.distortion, image.width, image.height);
    if !(0.0..=1.0).contains(&lu) || !(0.0..=1.0).contains(&lv) {
        return [0.0, 0.0, 0.0];
    }
    sample_bilinear_rgba16f_lens(
        &image.rgba16f,
        image.width,
        image.height,
        lu,
        lv,
        lens.chromatic_aberration,
    )
}

/// Samples a single developed linear RGB pixel from `image` at normalized cropped/oriented
/// coordinates `(u, v)` in `[0.0, 1.0]^2`, respecting `recipe.geometry` (including keystone),
/// `recipe.lens` (distortion + chromatic aberration), and Stages 0.5..=8 of `recipe`
/// (`evaluate_pre_advanced_color_linear`) so Advanced Color / Skin Tone pickers sample the
/// exact color entering Stage 8b/8c.
pub fn sample_linear_rgb_at_uv(
    image: &LinearImage,
    u: f32,
    v: f32,
    recipe: &DevelopRecipe,
) -> [f32; 3] {
    let raw = sample_raw_linear_rgb_at_uv(image, u, v, recipe);
    if raw == [0.0, 0.0, 0.0] {
        return raw;
    }
    recipe.compile().evaluate_pre_advanced_color_linear(raw)
}

/// Renders `image` through the full CPU develop pipeline (geometry/crop/keystone, optical lens
/// correction, linear Catmull-Rom downscaling, spatial detail, color science, optical light falloff,
/// crop-aware vignette, film grain, film curve tonemap, and sRGB encoding) into an in-memory
/// `[f32; 3]` sRGB buffer in `[0.0, 1.0]`.
pub fn render_developed_srgb_f32(
    image: &LinearImage,
    recipe: &DevelopRecipe,
    options: &ExportOptions,
) -> Result<(u32, u32, Vec<[f32; 3]>), DecodeError> {
    render_developed_srgb_f32_with_profiles_dir(image, recipe, options, None)
}

/// Variant of [`render_developed_srgb_f32`] resolving `"user:<slug>"` camera profiles from an
/// explicit `profiles_dir` (or `~/.config/rune-darkroom/profiles` when `None`).
pub fn render_developed_srgb_f32_with_profiles_dir(
    image: &LinearImage,
    recipe: &DevelopRecipe,
    options: &ExportOptions,
    profiles_dir: Option<&Path>,
) -> Result<(u32, u32, Vec<[f32; 3]>), DecodeError> {
    validate_linear_image(image)?;
    let prep = prepare_linear_source(image, recipe, options);
    let src = match prep.owned.as_deref() {
        Some(buf) => LinearSource::RgbF32(buf),
        None => LinearSource::Rgba16F(&image.rgba16f),
    };
    let w = prep.out_w as usize;
    let h = prep.out_h as usize;
    let mut srgb = vec![[0.0f32; 3]; w * h];
    render_rows_par(
        &mut srgb,
        w,
        src,
        prep.out_w,
        prep.out_h,
        prep.detail_scale,
        prep.frame_meta,
        recipe,
        options.color_space,
        profiles_dir,
        |row, x, px| {
            row[x] = px;
        },
    );
    if options.watermark.is_active() {
        composite_text_watermark_srgb(&mut srgb, w, h, &options.watermark);
    }
    Ok((prep.out_w, prep.out_h, srgb))
}

/// Renders `image` through the full develop pipeline (geometry/crop, linear Catmull-Rom
/// downscaling, spatial detail, color science, optical vignette, film grain, filmic tonemap,
/// and sRGB encoding) directly into an in-memory JPEG byte buffer.
pub fn export_image_to_jpeg_vec(
    image: &LinearImage,
    recipe: &DevelopRecipe,
    max_edge: u32,
    quality: u8,
) -> Result<Vec<u8>, DecodeError> {
    validate_linear_image(image)?;

    let quality = quality.clamp(1, 100);
    let options = ExportOptions {
        format: ExportFormat::Jpeg { quality },
        max_edge: (max_edge > 0).then_some(max_edge),
        respect_crop: true,
        output_sharpen: 0.0,
        ..ExportOptions::default()
    };

    let prep = prepare_linear_source(image, recipe, &options);
    let src = match prep.owned.as_deref() {
        Some(buf) => LinearSource::RgbF32(buf),
        None => LinearSource::Rgba16F(&image.rgba16f),
    };

    let rgb8 = render_export_quantized(
        src,
        prep.out_w,
        prep.out_h,
        prep.detail_scale,
        prep.frame_meta,
        recipe,
        0.0,
        options.color_space,
        &options.watermark,
        to_u8,
    );
    let mut out = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut out, quality);
    encoder
        .encode(&rgb8, prep.out_w, prep.out_h, ExtendedColorType::Rgb8)
        .map_err(|source| DecodeError::Image {
            path: image
                .source_path
                .clone()
                .unwrap_or_else(|| PathBuf::from(&image.display_name)),
            source,
        })?;
    Ok(out)
}

/// Evaluates `image` through `recipe` at full resolution and writes the encoded
/// result to `dest` in the requested [`ExportFormat`].
pub fn export_image(
    image: &LinearImage,
    recipe: &DevelopRecipe,
    dest: &Path,
    format: ExportFormat,
) -> Result<(), DecodeError> {
    export_image_with_options(
        image,
        recipe,
        dest,
        &ExportOptions {
            format,
            ..ExportOptions::default()
        },
    )
}

/// Evaluates `image` through `recipe` with full [`ExportOptions`] (geometry/crop,
/// high-quality downscaling, spatial detail, output sharpening, color space, watermark,
/// and format encoding) and writes to `dest`.
pub fn export_image_with_options(
    image: &LinearImage,
    recipe: &DevelopRecipe,
    dest: &Path,
    options: &ExportOptions,
) -> Result<(), DecodeError> {
    export_image_with_options_dims(image, recipe, dest, options).map(|_| ())
}

fn export_image_with_options_dims(
    image: &LinearImage,
    recipe: &DevelopRecipe,
    dest: &Path,
    options: &ExportOptions,
) -> Result<(u32, u32), DecodeError> {
    validate_linear_image(image)?;

    // Step 1: Optional linear-space geometry/crop/keystone/lens and Catmull-Rom downscaling.
    let prep = prepare_linear_source(image, recipe, options);
    let out_w = prep.out_w;
    let out_h = prep.out_h;
    let detail_scale = prep.detail_scale;
    let frame_meta = prep.frame_meta;
    let src = match prep.owned.as_deref() {
        Some(buf) => LinearSource::RgbF32(buf),
        None => LinearSource::Rgba16F(&image.rgba16f),
    };

    let sharpen = if options.output_sharpen.is_finite() {
        options.output_sharpen.clamp(0.0, 1.0)
    } else {
        0.0
    };

    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|source| DecodeError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let parent = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let fname = dest
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "export.bin".to_owned());
    let seq = EXPORT_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_path = parent.join(format!(".{fname}.tmp.{}.{seq}", std::process::id()));

    let encode_result = (|| -> Result<(), DecodeError> {
        let file = File::create(&tmp_path).map_err(|source| DecodeError::Io {
            path: tmp_path.clone(),
            source,
        })?;
        let mut writer = BufWriter::new(file);

        match options.format {
            ExportFormat::Jpeg { quality } => {
                let rgb8 = render_export_quantized(
                    src,
                    out_w,
                    out_h,
                    detail_scale,
                    frame_meta,
                    recipe,
                    sharpen,
                    options.color_space,
                    &options.watermark,
                    to_u8,
                );
                let mut encoder = JpegEncoder::new_with_quality(&mut writer, quality.clamp(1, 100));
                encoder
                    .encode(&rgb8, out_w, out_h, ExtendedColorType::Rgb8)
                    .map_err(|source| DecodeError::Image {
                        path: dest.to_path_buf(),
                        source,
                    })?;
            }
            ExportFormat::Png8 => {
                let rgb8 = render_export_quantized(
                    src,
                    out_w,
                    out_h,
                    detail_scale,
                    frame_meta,
                    recipe,
                    sharpen,
                    options.color_space,
                    &options.watermark,
                    to_u8,
                );
                let buf: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_raw(out_w, out_h, rgb8)
                    .ok_or_else(|| {
                        DecodeError::InvalidBuffer(
                            "Failed to construct Rgb8 ImageBuffer".to_owned(),
                        )
                    })?;
                buf.write_to(&mut writer, ImageFormat::Png)
                    .map_err(|source| DecodeError::Image {
                        path: dest.to_path_buf(),
                        source,
                    })?;
            }
            ExportFormat::Png16 => {
                let rgb16 = render_export_quantized(
                    src,
                    out_w,
                    out_h,
                    detail_scale,
                    frame_meta,
                    recipe,
                    sharpen,
                    options.color_space,
                    &options.watermark,
                    to_u16,
                );
                let buf: ImageBuffer<Rgb<u16>, Vec<u16>> =
                    ImageBuffer::from_raw(out_w, out_h, rgb16).ok_or_else(|| {
                        DecodeError::InvalidBuffer(
                            "Failed to construct Rgb16 ImageBuffer".to_owned(),
                        )
                    })?;
                buf.write_to(&mut writer, ImageFormat::Png)
                    .map_err(|source| DecodeError::Image {
                        path: dest.to_path_buf(),
                        source,
                    })?;
            }
            ExportFormat::Tiff16 => {
                let rgb16 = render_export_quantized(
                    src,
                    out_w,
                    out_h,
                    detail_scale,
                    frame_meta,
                    recipe,
                    sharpen,
                    options.color_space,
                    &options.watermark,
                    to_u16,
                );
                let buf: ImageBuffer<Rgb<u16>, Vec<u16>> =
                    ImageBuffer::from_raw(out_w, out_h, rgb16).ok_or_else(|| {
                        DecodeError::InvalidBuffer(
                            "Failed to construct Rgb16 ImageBuffer".to_owned(),
                        )
                    })?;
                buf.write_to(&mut writer, ImageFormat::Tiff)
                    .map_err(|source| DecodeError::Image {
                        path: dest.to_path_buf(),
                        source,
                    })?;
            }
            ExportFormat::WebP => {
                let rgb8 = render_export_quantized(
                    src,
                    out_w,
                    out_h,
                    detail_scale,
                    frame_meta,
                    recipe,
                    sharpen,
                    options.color_space,
                    &options.watermark,
                    to_u8,
                );
                WebPEncoder::new_lossless(&mut writer)
                    .encode(&rgb8, out_w, out_h, ExtendedColorType::Rgb8)
                    .map_err(|source| DecodeError::Image {
                        path: dest.to_path_buf(),
                        source,
                    })?;
            }
        }

        writer.flush().map_err(|source| DecodeError::Io {
            path: tmp_path.clone(),
            source,
        })?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|source| DecodeError::Io {
                path: tmp_path.clone(),
                source,
            })?;
        Ok(())
    })();

    if let Err(err) = encode_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err);
    }

    std::fs::rename(&tmp_path, dest).map_err(|source| {
        let _ = std::fs::remove_file(&tmp_path);
        DecodeError::Io {
            path: dest.to_path_buf(),
            source,
        }
    })?;

    Ok((out_w, out_h))
}

#[derive(Clone, Copy)]
enum LinearSource<'a> {
    Rgba16F(&'a [f16]),
    RgbF32(&'a [[f32; 3]]),
}

impl LinearSource<'_> {
    #[inline]
    fn get_rgb(&self, idx: usize) -> [f32; 3] {
        match self {
            Self::Rgba16F(buf) => {
                let base = idx * 4;
                [
                    buf[base].to_f32(),
                    buf[base + 1].to_f32(),
                    buf[base + 2].to_f32(),
                ]
            }
            Self::RgbF32(buf) => buf[idx],
        }
    }

    #[inline]
    fn sample_clamped_px(&self, width: usize, height: usize, x: isize, y: isize) -> [f32; 3] {
        let cx = x.clamp(0, (width - 1) as isize) as usize;
        let cy = y.clamp(0, (height - 1) as isize) as usize;
        let p = self.get_rgb(cy * width + cx);
        [p[0].max(0.0), p[1].max(0.0), p[2].max(0.0)]
    }

    #[inline]
    fn sample_bilinear_px(&self, width: usize, height: usize, fx: f32, fy: f32) -> [f32; 3] {
        let cx = fx.clamp(0.0, (width - 1) as f32);
        let cy = fy.clamp(0.0, (height - 1) as f32);
        let x0 = cx.floor() as usize;
        let y0 = cy.floor() as usize;
        let x1 = (x0 + 1).min(width - 1);
        let y1 = (y0 + 1).min(height - 1);
        let tx = cx - x0 as f32;
        let ty = cy - y0 as f32;

        let p00 = self.get_rgb(y0 * width + x0);
        let p10 = self.get_rgb(y0 * width + x1);
        let p01 = self.get_rgb(y1 * width + x0);
        let p11 = self.get_rgb(y1 * width + x1);

        let w00 = (1.0 - tx) * (1.0 - ty);
        let w10 = tx * (1.0 - ty);
        let w01 = (1.0 - tx) * ty;
        let w11 = tx * ty;

        [
            (p00[0] * w00 + p10[0] * w10 + p01[0] * w01 + p11[0] * w11).max(0.0),
            (p00[1] * w00 + p10[1] * w10 + p01[1] * w01 + p11[1] * w11).max(0.0),
            (p00[2] * w00 + p10[2] * w10 + p01[2] * w01 + p11[2] * w11).max(0.0),
        ]
    }
}

#[derive(Clone, Copy)]
struct ExportFrameMeta {
    crop: [f32; 4],
    ori_size: [f32; 2],
    crop_size: [f32; 2],
    src_size: [u32; 2],
    respect_crop: bool,
    geo_or_lens_active: bool,
}

struct PreparedLinearSource {
    out_w: u32,
    out_h: u32,
    detail_scale: [f32; 2],
    frame_meta: ExportFrameMeta,
    owned: Option<Vec<[f32; 3]>>,
}

fn prepare_linear_source(
    image: &LinearImage,
    recipe: &DevelopRecipe,
    options: &ExportOptions,
) -> PreparedLinearSource {
    let mut cur_w = image.width;
    let mut cur_h = image.height;
    let mut owned: Option<Vec<[f32; 3]>> = None;

    let lens = recipe.lens.sanitized();
    let geo_active = options.respect_crop && !recipe.geometry.is_identity();
    let lens_warp_active = lens.distortion.abs() > 1e-6 || lens.chromatic_aberration.abs() > 1e-5;
    let heal_active = !recipe.heal_spots.is_empty();

    let (crop, [ori_w, ori_h]) = if options.respect_crop {
        (
            recipe.geometry.sanitized_crop(),
            recipe.geometry.oriented_size(image.width, image.height),
        )
    } else {
        ([0.0, 0.0, 1.0, 1.0], [image.width, image.height])
    };
    let frame_meta = ExportFrameMeta {
        crop,
        ori_size: [ori_w.max(1) as f32, ori_h.max(1) as f32],
        crop_size: [
            (ori_w as f32 * crop[2]).max(1.0),
            (ori_h as f32 * crop[3]).max(1.0),
        ],
        src_size: [image.width.max(1), image.height.max(1)],
        respect_crop: options.respect_crop,
        geo_or_lens_active: geo_active || lens.distortion.abs() > 1e-6,
    };

    if geo_active || lens_warp_active || heal_active {
        let (geo_w, geo_h, transformed) =
            apply_geometry_and_lens_linear(image, recipe, options.respect_crop);
        cur_w = geo_w;
        cur_h = geo_h;
        owned = Some(transformed);
    }

    let pre_resize_w = cur_w;
    let pre_resize_h = cur_h;

    if let Some(limit) = options.max_edge {
        let limit = limit.max(1);
        if cur_w.max(cur_h) > limit {
            let (dst_w, dst_h) = fit_dimensions(cur_w, cur_h, limit);
            let src = match owned.as_deref() {
                Some(buf) => LinearSource::RgbF32(buf),
                None => LinearSource::Rgba16F(&image.rgba16f),
            };
            let resized = resize_linear_catmull_rom(src, cur_w, cur_h, dst_w, dst_h);
            cur_w = dst_w;
            cur_h = dst_h;
            owned = Some(resized);
        }
    }

    let detail_scale = [
        cur_w as f32 / pre_resize_w.max(1) as f32,
        cur_h as f32 / pre_resize_h.max(1) as f32,
    ];

    PreparedLinearSource {
        out_w: cur_w,
        out_h: cur_h,
        detail_scale,
        frame_meta,
        owned,
    }
}

/// Applies persistent geometry (`rotate_90`, `flip_h`, `flip_v`, `straighten_deg`, `keystone_*`,
/// and `crop`) plus radial lens distortion and lateral chromatic aberration in scene-referred
/// linear light with exact UV parity to `shaders/develop.wgsl`.
fn apply_geometry_and_lens_linear(
    image: &LinearImage,
    recipe: &DevelopRecipe,
    respect_crop: bool,
) -> (u32, u32, Vec<[f32; 3]>) {
    let geo = &recipe.geometry;
    let lens = recipe.lens.sanitized();
    let [out_w, out_h] = if respect_crop {
        geo.cropped_output_size(image.width, image.height)
    } else {
        [image.width, image.height]
    };
    let ow = out_w as usize;
    let oh = out_h as usize;
    let owf = out_w as f32;
    let ohf = out_h as f32;

    let sanitized_spots: Vec<HealSpot> = recipe
        .heal_spots
        .iter()
        .take(MAX_HEAL_SPOTS)
        .map(HealSpot::sanitized)
        .collect();

    let mut out = vec![[0.0f32; 3]; ow * oh];
    out.par_chunks_exact_mut(ow)
        .enumerate()
        .for_each(|(y, dst_row)| {
            let out_v = (y as f32 + 0.5) / ohf;
            for (x, dst_px) in dst_row.iter_mut().enumerate() {
                let out_u = (x as f32 + 0.5) / owf;
                let [su, sv] = if respect_crop {
                    geo.cropped_to_source_uv_unclamped(out_u, out_v, image.width, image.height)
                } else {
                    [out_u, out_v]
                };
                let [lu, lv] =
                    apply_lens_distortion_uv([su, sv], lens.distortion, image.width, image.height);
                if (0.0..=1.0).contains(&lu) && (0.0..=1.0).contains(&lv) {
                    let mut px = sample_bilinear_rgba16f_lens(
                        &image.rgba16f,
                        image.width,
                        image.height,
                        lu,
                        lv,
                        lens.chromatic_aberration,
                    );
                    if !sanitized_spots.is_empty() {
                        px = apply_heal_spots_cpu(
                            px,
                            lu,
                            lv,
                            &image.rgba16f,
                            image.width,
                            image.height,
                            lens.chromatic_aberration,
                            &sanitized_spots,
                        );
                    }
                    *dst_px = px;
                }
            }
        });

    (out_w, out_h, out)
}

#[allow(clippy::too_many_arguments)]
#[inline]
fn apply_heal_spots_cpu(
    base_rgb: [f32; 3],
    u: f32,
    v: f32,
    rgba16f: &[f16],
    width: u32,
    height: u32,
    chromatic_aberration: f32,
    spots: &[HealSpot],
) -> [f32; 3] {
    let img_w = width.max(1) as f32;
    let img_h = height.max(1) as f32;
    let min_dim = img_w.min(img_h);
    let ax = img_w / min_dim;
    let ay = img_h / min_dim;

    let sample = |su: f32, sv: f32| {
        sample_bilinear_rgba16f_lens(
            rgba16f,
            width,
            height,
            su.clamp(0.0, 1.0),
            sv.clamp(0.0, 1.0),
            chromatic_aberration,
        )
    };

    let mut rgb = base_rgb;
    for spot in spots {
        if spot.opacity <= 1e-4 {
            continue;
        }
        let du = (u - spot.target[0]) * ax;
        let dv = (v - spot.target[1]) * ay;
        let dist = du.hypot(dv);
        if dist < spot.radius {
            let inner = spot.radius * (1.0 - spot.feather);
            let mut w = if spot.radius - inner > 1e-6 {
                1.0 - smoothstep(inner, spot.radius, dist)
            } else {
                1.0
            };
            w *= spot.opacity;
            let src_u = (spot.source[0] + (u - spot.target[0])).clamp(0.0, 1.0);
            let src_v = (spot.source[1] + (v - spot.target[1])).clamp(0.0, 1.0);
            let donor = sample(src_u, src_v);
            match spot.mode {
                HealMode::Clone => {
                    for ch in 0..3 {
                        rgb[ch] = lerp_f32(rgb[ch], donor[ch], w);
                    }
                }
                HealMode::Heal => {
                    let step_u = (spot.radius * 1.15) / ax;
                    let step_v = (spot.radius * 1.15) / ay;
                    let t_ring = 0.25
                        * (luma709(sample(spot.target[0] + step_u, spot.target[1]))
                            + luma709(sample(spot.target[0] - step_u, spot.target[1]))
                            + luma709(sample(spot.target[0], spot.target[1] + step_v))
                            + luma709(sample(spot.target[0], spot.target[1] - step_v)));
                    let s_ring = 0.25
                        * (luma709(sample(spot.source[0] + step_u, spot.source[1]))
                            + luma709(sample(spot.source[0] - step_u, spot.source[1]))
                            + luma709(sample(spot.source[0], spot.source[1] + step_v))
                            + luma709(sample(spot.source[0], spot.source[1] - step_v)));
                    let ratio = (t_ring / s_ring.max(1e-4)).clamp(0.25, 4.0);
                    for ch in 0..3 {
                        rgb[ch] = lerp_f32(rgb[ch], (donor[ch] * ratio).max(0.0), w);
                    }
                }
            }
        }
    }
    rgb
}

#[inline]
fn sample_bilinear_rgba16f_lens(
    rgba16f: &[f16],
    width: u32,
    height: u32,
    u: f32,
    v: f32,
    chromatic_aberration: f32,
) -> [f32; 3] {
    if chromatic_aberration.abs() <= 1e-5 {
        return sample_bilinear_rgba16f(rgba16f, width, height, u, v);
    }
    let ca = chromatic_aberration.clamp(-1.0, 1.0) * 0.008;
    let du = u - 0.5;
    let dv = v - 0.5;
    let r = sample_bilinear_rgba16f(
        rgba16f,
        width,
        height,
        0.5 + du * (1.0 + ca),
        0.5 + dv * (1.0 + ca),
    )[0];
    let g = sample_bilinear_rgba16f(rgba16f, width, height, u, v)[1];
    let b = sample_bilinear_rgba16f(
        rgba16f,
        width,
        height,
        0.5 + du * (1.0 - ca),
        0.5 + dv * (1.0 - ca),
    )[2];
    [r, g, b]
}

#[inline]
fn sample_bilinear_rgba16f(rgba16f: &[f16], width: u32, height: u32, u: f32, v: f32) -> [f32; 3] {
    let w = width as usize;
    let h = height as usize;
    let fx = (u * width as f32 - 0.5).clamp(0.0, (w - 1) as f32);
    let fy = (v * height as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let x0 = fx.floor() as usize;
    let y0 = fy.floor() as usize;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;

    let read = |x: usize, y: usize| -> [f32; 3] {
        let i = (y * w + x) * 4;
        [
            rgba16f[i].to_f32(),
            rgba16f[i + 1].to_f32(),
            rgba16f[i + 2].to_f32(),
        ]
    };
    let p00 = read(x0, y0);
    let p10 = read(x1, y0);
    let p01 = read(x0, y1);
    let p11 = read(x1, y1);

    let w00 = (1.0 - tx) * (1.0 - ty);
    let w10 = tx * (1.0 - ty);
    let w01 = (1.0 - tx) * ty;
    let w11 = tx * ty;

    [
        (p00[0] * w00 + p10[0] * w10 + p01[0] * w01 + p11[0] * w11).max(0.0),
        (p00[1] * w00 + p10[1] * w10 + p01[1] * w01 + p11[1] * w11).max(0.0),
        (p00[2] * w00 + p10[2] * w10 + p01[2] * w01 + p11[2] * w11).max(0.0),
    ]
}

/// High-quality separable Catmull-Rom downscaler operating in scene-referred linear light.
///
/// Scales the filter support radius by `src_dim / dst_dim` when downscaling so large
/// resize ratios stay alias-free while preserving crisp micro-contrast.
fn resize_linear_catmull_rom(
    src: LinearSource<'_>,
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> Vec<[f32; 3]> {
    let sw = src_w as usize;
    let sh = src_h as usize;
    let dw = dst_w as usize;
    let dh = dst_h as usize;

    let x_taps = build_catmull_rom_taps(sw, dw);
    let y_taps = build_catmull_rom_taps(sh, dh);

    // Pass 1: Horizontal resample (`sh` rows x `dw` cols).
    let mut horiz = vec![[0.0f32; 3]; sh * dw];
    horiz
        .par_chunks_exact_mut(dw)
        .enumerate()
        .for_each(|(sy, dst_row)| {
            let row_offset = sy * sw;
            for (dx, dst_px) in dst_row.iter_mut().enumerate() {
                let taps = &x_taps[dx];
                let mut acc = [0.0f32; 3];
                for &(sx, w) in taps {
                    let p = src.get_rgb(row_offset + sx);
                    acc[0] += p[0] * w;
                    acc[1] += p[1] * w;
                    acc[2] += p[2] * w;
                }
                *dst_px = [acc[0].max(0.0), acc[1].max(0.0), acc[2].max(0.0)];
            }
        });

    // Pass 2: Vertical resample (`dh` rows x `dw` cols).
    let mut out = vec![[0.0f32; 3]; dh * dw];
    out.par_chunks_exact_mut(dw)
        .enumerate()
        .for_each(|(dy, dst_row)| {
            let taps = &y_taps[dy];
            for (dx, dst_px) in dst_row.iter_mut().enumerate() {
                let mut acc = [0.0f32; 3];
                for &(sy, w) in taps {
                    let p = horiz[sy * dw + dx];
                    acc[0] += p[0] * w;
                    acc[1] += p[1] * w;
                    acc[2] += p[2] * w;
                }
                *dst_px = [acc[0].max(0.0), acc[1].max(0.0), acc[2].max(0.0)];
            }
        });

    out
}

fn build_catmull_rom_taps(src_len: usize, dst_len: usize) -> Vec<Vec<(usize, f32)>> {
    let scale = (src_len as f32 / dst_len.max(1) as f32).max(1.0);
    let support = 2.0 * scale;
    let inv_scale = 1.0 / scale;
    let max_idx = src_len.saturating_sub(1) as isize;

    (0..dst_len)
        .map(|dst_i| {
            let center = (dst_i as f32 + 0.5) * (src_len as f32 / dst_len as f32) - 0.5;
            let start = (center - support).floor() as isize;
            let end = (center + support).ceil() as isize;
            let mut taps = Vec::with_capacity((end - start + 1).max(1) as usize);
            let mut w_sum = 0.0f32;

            for si in start..=end {
                let dist = ((si as f32) - center) * inv_scale;
                let w = catmull_rom_weight(dist);
                if w.abs() > 1e-6 {
                    let clamped = si.clamp(0, max_idx) as usize;
                    taps.push((clamped, w));
                    w_sum += w;
                }
            }

            if w_sum.abs() > 1e-6 {
                let norm = 1.0 / w_sum;
                for tap in &mut taps {
                    tap.1 *= norm;
                }
            } else {
                let nearest = (center.round() as isize).clamp(0, max_idx) as usize;
                taps.clear();
                taps.push((nearest, 1.0));
            }
            taps
        })
        .collect()
}

#[inline]
fn catmull_rom_weight(x: f32) -> f32 {
    let ax = x.abs();
    if ax < 1.0 {
        1.5 * ax * ax * ax - 2.5 * ax * ax + 1.0
    } else if ax < 2.0 {
        -0.5 * ax * ax * ax + 2.5 * ax * ax - 4.0 * ax + 2.0
    } else {
        0.0
    }
}

#[inline]
fn to_u8(v: f32) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

#[inline]
fn to_u16(v: f32) -> u16 {
    (v * 65535.0).round().clamp(0.0, 65535.0) as u16
}

#[derive(Clone, Copy)]
struct SpatialDetailParams {
    clarity: f32,
    clarity_method: ClarityMethod,
    sharpen: f32,
    sharpen_radius: f32,
    sharpen_threshold: f32,
    sharpen_halo: f32,
    nr: f32,
    nr_color: f32,
    structure: f32,
    purple_fringing: f32,
    active: bool,
}

impl SpatialDetailParams {
    fn from_recipe(recipe: &DevelopRecipe) -> Self {
        let clamp_signed = |v: f32| {
            if v.is_finite() {
                v.clamp(-1.0, 1.0)
            } else {
                0.0
            }
        };
        let clamp_pos = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        let clarity = clamp_signed(recipe.clarity);
        let sharpen = clamp_pos(recipe.sharpen);
        let sharpen_radius = if recipe.sharpen_radius.is_finite() {
            recipe.sharpen_radius.clamp(0.5, 4.0)
        } else {
            1.0
        };
        let sharpen_threshold = clamp_pos(recipe.sharpen_threshold);
        let sharpen_halo = clamp_pos(recipe.sharpen_halo);
        let nr = clamp_pos(recipe.noise_reduction);
        let nr_color = clamp_pos(recipe.nr_color);
        let structure = clamp_signed(recipe.structure);
        let purple_fringing = clamp_pos(recipe.lens.purple_fringing);
        let active = clarity.abs() > 0.001
            || sharpen > 0.001
            || nr > 0.001
            || nr_color > 0.001
            || structure.abs() > 0.001
            || purple_fringing > 0.001;
        Self {
            clarity,
            clarity_method: recipe.clarity_method,
            sharpen,
            sharpen_radius,
            sharpen_threshold,
            sharpen_halo,
            nr,
            nr_color,
            structure,
            purple_fringing,
            active,
        }
    }
}

#[inline]
fn luma709(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

#[inline]
fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// Evaluates Step 0 linear spatial detail (`noise_reduction`, `nr_color`, `clarity` + `clarity_method`,
/// `structure`, `sharpen` + `radius`/`threshold`/`halo`) matching `apply_spatial_detail` in
/// `shaders/develop.wgsl` 1:1 at full resolution, and scaling broad/medium tap radii proportionally
/// when rendering downscaled exports/previews.
#[inline]
fn apply_spatial_detail_pixel(
    src: LinearSource<'_>,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    detail: SpatialDetailParams,
    radius_scale: [f32; 2],
) -> [f32; 3] {
    let center = src.get_rgb(y * width + x);
    let mut rgb = [center[0].max(0.0), center[1].max(0.0), center[2].max(0.0)];
    if !detail.active || width < 2 || height < 2 {
        return rgb;
    }

    let xi = x as isize;
    let yi = y as isize;
    let xf = x as f32;
    let yf = y as f32;
    let sx = radius_scale[0].clamp(0.0, 1.0);
    let sy = radius_scale[1].clamp(0.0, 1.0);

    // Tight 4-tap cardinal samples (fetched only when NR, Structure, or 1px Sharpening is active).
    let mut n_up = [0.0f32; 3];
    let mut n_dn = [0.0f32; 3];
    let mut n_lt = [0.0f32; 3];
    let mut n_rt = [0.0f32; 3];
    let need_tight_taps = detail.nr > 0.001
        || detail.nr_color > 0.001
        || detail.structure.abs() > 0.001
        || detail.purple_fringing > 0.001
        || (detail.sharpen > 0.001 && (detail.sharpen_radius - 1.0).abs() <= 0.01);
    if need_tight_taps {
        n_up = src.sample_clamped_px(width, height, xi, yi - 1);
        n_dn = src.sample_clamped_px(width, height, xi, yi + 1);
        n_lt = src.sample_clamped_px(width, height, xi - 1, yi);
        n_rt = src.sample_clamped_px(width, height, xi + 1, yi);
    }

    // 1. Edge-aware bilateral Chroma + Luma Noise Reduction
    if detail.nr > 0.001 || detail.nr_color > 0.001 {
        let dx = (1.25 * sx).max(1.0);
        let dy = (1.25 * sy).max(1.0);
        let d0 = src.sample_bilinear_px(width, height, xf - dx, yf - dy);
        let d1 = src.sample_bilinear_px(width, height, xf + dx, yf - dy);
        let d2 = src.sample_bilinear_px(width, height, xf - dx, yf + dy);
        let d3 = src.sample_bilinear_px(width, height, xf + dx, yf + dy);

        let c_p = luma709(rgb).max(0.0).sqrt();
        let nr_eff = detail.nr.max(detail.nr_color);
        let sigma = 0.04 + nr_eff * 0.14;
        let inv_2s2 = 0.5 / (sigma * sigma);

        let mut sum_rgb = rgb;
        let mut sum_w = 1.0f32;
        for s_rgb in [n_up, n_dn, n_lt, n_rt, d0, d1, d2, d3] {
            let diff = luma709(s_rgb).max(0.0).sqrt() - c_p;
            let w = (-(diff * diff) * inv_2s2).exp();
            sum_rgb[0] += s_rgb[0] * w;
            sum_rgb[1] += s_rgb[1] * w;
            sum_rgb[2] += s_rgb[2] * w;
            sum_w += w;
        }
        let inv_w = 1.0 / sum_w.max(1e-5);
        let denoised = [sum_rgb[0] * inv_w, sum_rgb[1] * inv_w, sum_rgb[2] * inv_w];

        let y_c = luma709(rgb);
        let y_d = luma709(denoised);
        let luma_mix = detail.nr * 0.42;
        let chroma_mix = 1.0 - (1.0 - detail.nr * 0.85) * (1.0 - detail.nr_color * 0.95);
        let y_out = y_c + (y_d - y_c) * luma_mix;
        for ch in 0..3 {
            let chroma_c = rgb[ch] - y_c;
            let chroma_d = denoised[ch] - y_d;
            let chroma_out = chroma_c + (chroma_d - chroma_c) * chroma_mix;
            rgb[ch] = (y_out + chroma_out).max(0.0);
        }
    }

    // 2. Spatial Clarity (broad-radius local contrast with Natural / Punch / Neutral / Classic)
    if detail.clarity.abs() > 0.001 {
        let r1x = (3.0 * sx).max(1.0);
        let r1y = (3.0 * sy).max(1.0);
        let r2x = (7.0 * sx).max(2.0);
        let r2y = (7.0 * sy).max(2.0);

        let b1 = add3(
            add3(
                src.sample_bilinear_px(width, height, xf + r1x, yf),
                src.sample_bilinear_px(width, height, xf - r1x, yf),
            ),
            add3(
                src.sample_bilinear_px(width, height, xf, yf + r1y),
                src.sample_bilinear_px(width, height, xf, yf - r1y),
            ),
        );
        let b2 = add3(
            add3(
                src.sample_bilinear_px(width, height, xf + r2x, yf + r2y),
                src.sample_bilinear_px(width, height, xf - r2x, yf + r2y),
            ),
            add3(
                src.sample_bilinear_px(width, height, xf + r2x, yf - r2y),
                src.sample_bilinear_px(width, height, xf - r2x, yf - r2y),
            ),
        );

        let wide_avg = [
            ((rgb[0] * 2.0 + b1[0] + b2[0] * 0.75) / 9.0).max(0.0),
            ((rgb[1] * 2.0 + b1[1] + b2[1] * 0.75) / 9.0).max(0.0),
            ((rgb[2] * 2.0 + b1[2] + b2[2] * 0.75) / 9.0).max(0.0),
        ];
        let y_c = luma709(rgb).max(1e-4);
        let y_b = luma709(wide_avg).max(1e-4);
        let t = (y_c / (y_c + 0.18)).clamp(0.0, 1.0);
        let mid_mask = 4.0 * t * (1.0 - t);

        match detail.clarity_method {
            ClarityMethod::Punch => {
                let ratio = (y_c / y_b).clamp(0.25, 4.0);
                let mask = 0.25 + 0.75 * mid_mask;
                let gain = ratio.powf(detail.clarity * 0.88 * mask);
                rgb[0] = (rgb[0] * gain).max(0.0);
                rgb[1] = (rgb[1] * gain).max(0.0);
                rgb[2] = (rgb[2] * gain).max(0.0);
                let y_p = luma709(rgb);
                let sat_boost = (1.0 + detail.clarity * 0.20 * mid_mask).max(0.0);
                for ch in &mut rgb {
                    *ch = (y_p + (*ch - y_p) * sat_boost).max(0.0);
                }
            }
            ClarityMethod::Neutral => {
                let ratio = (y_c / y_b).clamp(0.25, 4.0);
                let gain = ratio.powf(detail.clarity * 0.62 * mid_mask);
                let luma_delta = y_c * (gain - 1.0);
                for ch in &mut rgb {
                    let mul_v = (*ch * gain).max(0.0);
                    let add_v = (*ch + luma_delta).max(0.0);
                    *ch = mul_v + (add_v - mul_v) * 0.65;
                }
            }
            ClarityMethod::Classic => {
                let ratio = (y_c / y_b).clamp(0.35, 3.0);
                let mask = 0.45 + 0.55 * mid_mask;
                let gain = ratio.powf(detail.clarity * 0.58 * mask);
                rgb[0] = (rgb[0] * gain).max(0.0);
                rgb[1] = (rgb[1] * gain).max(0.0);
                rgb[2] = (rgb[2] * gain).max(0.0);
            }
            ClarityMethod::Natural => {
                let ratio = (y_c / y_b).clamp(0.25, 4.0);
                let gain = ratio.powf(detail.clarity * 0.65 * mid_mask);
                rgb[0] = (rgb[0] * gain).max(0.0);
                rgb[1] = (rgb[1] * gain).max(0.0);
                rgb[2] = (rgb[2] * gain).max(0.0);
            }
        }
    }

    // 3. Spatial Structure (fine-radius micro-contrast texture enhancement)
    if detail.structure.abs() > 0.001 {
        let rsx = (1.65 * sx).max(1.0);
        let rsy = (1.65 * sy).max(1.0);
        let s_diag = add3(
            add3(
                src.sample_bilinear_px(width, height, xf + rsx, yf + rsy),
                src.sample_bilinear_px(width, height, xf - rsx, yf + rsy),
            ),
            add3(
                src.sample_bilinear_px(width, height, xf + rsx, yf - rsy),
                src.sample_bilinear_px(width, height, xf - rsx, yf - rsy),
            ),
        );
        let cardinal_sum = add3(add3(n_up, n_dn), add3(n_lt, n_rt));
        let fine_avg = [
            ((rgb[0] * 2.0 + cardinal_sum[0] + s_diag[0] * 0.75) / 9.0).max(0.0),
            ((rgb[1] * 2.0 + cardinal_sum[1] + s_diag[1] * 0.75) / 9.0).max(0.0),
            ((rgb[2] * 2.0 + cardinal_sum[2] + s_diag[2] * 0.75) / 9.0).max(0.0),
        ];
        let y_c = luma709(rgb).max(1e-4);
        let y_f = luma709(fine_avg).max(1e-4);
        let shadow_guard = smoothstep(0.004, 0.05, y_c);
        let ratio = (y_c / y_f).clamp(0.35, 2.8);
        let gain = ratio.powf(detail.structure * 0.72 * shadow_guard);
        rgb[0] = (rgb[0] * gain).max(0.0);
        rgb[1] = (rgb[1] * gain).max(0.0);
        rgb[2] = (rgb[2] * gain).max(0.0);
    }

    // 4. Unsharp Mask Sharpening (Radius, Threshold, Halo Suppression)
    if detail.sharpen > 0.001 {
        let blur_tight = if (detail.sharpen_radius - 1.0).abs() <= 0.01 {
            let cardinal_sum = add3(add3(n_up, n_dn), add3(n_lt, n_rt));
            [
                cardinal_sum[0] * 0.25,
                cardinal_sum[1] * 0.25,
                cardinal_sum[2] * 0.25,
            ]
        } else {
            let srx = (detail.sharpen_radius * sx).max(0.5);
            let sry = (detail.sharpen_radius * sy).max(0.5);
            let s_up = src.sample_bilinear_px(width, height, xf, yf - sry);
            let s_dn = src.sample_bilinear_px(width, height, xf, yf + sry);
            let s_lt = src.sample_bilinear_px(width, height, xf - srx, yf);
            let s_rt = src.sample_bilinear_px(width, height, xf + srx, yf);
            let sum = add3(add3(s_up, s_dn), add3(s_lt, s_rt));
            [sum[0] * 0.25, sum[1] * 0.25, sum[2] * 0.25]
        };
        let y_c = luma709(rgb);
        let y_blur = luma709(blur_tight);
        let hp = y_c - y_blur;
        let th_lo = 0.002 + detail.sharpen_threshold * 0.048;
        let th_hi = th_lo + 0.010 + detail.sharpen_threshold * 0.030;
        let edge_gate = smoothstep(th_lo, th_hi, hp.abs());
        let rel_clamped = (hp / y_c.max(0.02)).clamp(-0.55, 0.55);
        let halo_limit = 0.55 - detail.sharpen_halo * 0.35;
        let halo_damp = 1.0 / (1.0 + detail.sharpen_halo * rel_clamped.abs() * 5.0);
        let rel_hp = (rel_clamped * halo_damp).clamp(-halo_limit, halo_limit);
        let sharp_gain = (1.0 + rel_hp * detail.sharpen * 1.4 * edge_gate).max(0.0);
        rgb[0] *= sharp_gain;
        rgb[1] *= sharp_gain;
        rgb[2] *= sharp_gain;
    }

    // 5. Axial Purple/Magenta Edge Fringing Suppression
    if detail.purple_fringing > 0.001 {
        let y_c = luma709(rgb);
        let y_up = luma709(n_up);
        let y_dn = luma709(n_dn);
        let y_lt = luma709(n_lt);
        let y_rt = luma709(n_rt);
        let max_n = y_up.max(y_dn).max(y_lt.max(y_rt));
        let min_n = y_up.min(y_dn).min(y_lt.min(y_rt));
        let edge_contrast = smoothstep(0.06, 0.30, max_n.max(y_c) - min_n.min(y_c));
        if edge_contrast > 1e-4 {
            let (h_deg, s, _) = rune_editor_core::sample_color_slice_from_rgb(rgb);
            let diff_p = ((h_deg - 295.0 + 540.0).rem_euclid(360.0) - 180.0).abs();
            let purple_w = (1.0 - smoothstep(22.0, 52.0, diff_p)) * smoothstep(0.18, 0.45, s);
            let fringe_mask = (purple_w * edge_contrast * detail.purple_fringing).clamp(0.0, 1.0);
            let k = fringe_mask * 0.92;
            rgb[0] = lerp_f32(rgb[0], y_c, k);
            rgb[1] = lerp_f32(rgb[1], y_c, k);
            rgb[2] = lerp_f32(rgb[2], y_c, k);
        }
    }

    rgb
}

#[allow(clippy::too_many_arguments)]
fn render_export_quantized<T: Copy + Default + Send + Sync>(
    src: LinearSource<'_>,
    width: u32,
    height: u32,
    detail_scale: [f32; 2],
    frame_meta: ExportFrameMeta,
    recipe: &DevelopRecipe,
    sharpen: f32,
    color_space: ExportColorSpace,
    watermark: &TextWatermark,
    quantize: impl Fn(f32) -> T + Sync,
) -> Vec<T> {
    let w = width as usize;
    let h = height as usize;
    let do_sharpen = sharpen > 0.001 && w >= 2 && h >= 2;
    let do_watermark = watermark.is_active();

    if do_sharpen || do_watermark {
        let mut srgb = vec![[0.0f32; 3]; w * h];
        render_rows_par(
            &mut srgb,
            w,
            src,
            width,
            height,
            detail_scale,
            frame_meta,
            recipe,
            color_space,
            None,
            |row, x, px| {
                row[x] = px;
            },
        );
        if do_sharpen && !do_watermark {
            return sharpen_and_quantize(&srgb, w, h, sharpen, quantize);
        }
        if do_sharpen {
            srgb = sharpen_srgb_buffer(&srgb, w, h, sharpen);
        }
        if do_watermark {
            composite_text_watermark_srgb(&mut srgb, w, h, watermark);
        }
        let mut out = vec![T::default(); w * h * 3];
        out.par_chunks_exact_mut(w * 3)
            .enumerate()
            .for_each(|(y, dst_row)| {
                let row_base = y * w;
                for x in 0..w {
                    let px = srgb[row_base + x];
                    let didx = x * 3;
                    dst_row[didx] = quantize(px[0]);
                    dst_row[didx + 1] = quantize(px[1]);
                    dst_row[didx + 2] = quantize(px[2]);
                }
            });
        return out;
    }

    let mut out = vec![T::default(); w * h * 3];
    render_rows_par(
        &mut out,
        w * 3,
        src,
        width,
        height,
        detail_scale,
        frame_meta,
        recipe,
        color_space,
        None,
        |row, x, px| {
            let didx = x * 3;
            row[didx] = quantize(px[0]);
            row[didx + 1] = quantize(px[1]);
            row[didx + 2] = quantize(px[2]);
        },
    );
    out
}

#[allow(clippy::too_many_arguments)]
fn render_rows_par<T: Send>(
    out: &mut [T],
    row_stride: usize,
    src: LinearSource<'_>,
    width: u32,
    height: u32,
    detail_scale: [f32; 2],
    frame_meta: ExportFrameMeta,
    recipe: &DevelopRecipe,
    color_space: ExportColorSpace,
    profiles_dir: Option<&Path>,
    write_px: impl Fn(&mut [T], usize, [f32; 3]) + Sync,
) {
    let compiled = recipe.compile_with_profiles_dir(profiles_dir);
    let spatial = SpatialDetailParams::from_recipe(recipe);
    let sanitized_layers: Vec<AdjustmentLayer> = recipe
        .layers
        .iter()
        .take(MAX_ADJUSTMENT_LAYERS)
        .map(AdjustmentLayer::sanitized)
        .collect();
    let layer_atlas = if sanitized_layers.is_empty() {
        None
    } else {
        Some(bake_layer_mask_atlas(&sanitized_layers))
    };
    let need_local_detail = sanitized_layers.iter().any(|l| {
        l.enabled
            && l.opacity > 1e-5
            && (l.adjustments.clarity.abs() > 1e-5 || l.adjustments.sharpen > 1e-4)
    });
    let w = width as usize;
    let h = height as usize;
    let wf = width.max(1) as f32;
    let hf = height.max(1) as f32;
    let sx = detail_scale[0].max(0.05);
    let sy = detail_scale[1].max(0.05);

    out.par_chunks_exact_mut(row_stride)
        .enumerate()
        .for_each(|(y, dst_row)| {
            let row_base = y * w;
            let v = (y as f32 + 0.5) / hf;
            let yf = y as f32;
            for x in 0..w {
                let u = (x as f32 + 0.5) / wf;
                let raw_px = src.get_rgb(row_base + x);
                let base_rgb = if spatial.active {
                    apply_spatial_detail_pixel(src, w, h, x, y, spatial, detail_scale)
                } else {
                    raw_px
                };
                let (local_mid_ratio, local_hp_rel) = if need_local_detail {
                    let xf = x as f32;
                    let y_src = luma709(raw_px).max(1e-5);
                    let y_tight = 0.25
                        * (luma709(src.sample_bilinear_px(w, h, xf, yf - sy))
                            + luma709(src.sample_bilinear_px(w, h, xf, yf + sy))
                            + luma709(src.sample_bilinear_px(w, h, xf - sx, yf))
                            + luma709(src.sample_bilinear_px(w, h, xf + sx, yf)));
                    let d2x = sx * 2.5;
                    let d2y = sy * 2.5;
                    let y_mid = 0.25
                        * (luma709(src.sample_bilinear_px(w, h, xf - d2x, yf - d2y))
                            + luma709(src.sample_bilinear_px(w, h, xf + d2x, yf - d2y))
                            + luma709(src.sample_bilinear_px(w, h, xf - d2x, yf + d2y))
                            + luma709(src.sample_bilinear_px(w, h, xf + d2x, yf + d2y)));
                    (
                        (y_src + 0.02) / (y_mid + 0.02),
                        (y_src - y_tight) / y_src.max(0.02),
                    )
                } else {
                    (1.0, 0.0)
                };
                let srgb = evaluate_export_pixel_srgb(
                    base_rgb,
                    u,
                    v,
                    x as f32,
                    y as f32,
                    frame_meta,
                    recipe,
                    &compiled,
                    color_space,
                    &sanitized_layers,
                    layer_atlas.as_ref(),
                    local_mid_ratio,
                    local_hp_rel,
                );
                write_px(dst_row, x, srgb);
            }
        });
}

fn sharpen_srgb_buffer(src: &[[f32; 3]], w: usize, h: usize, amount: f32) -> Vec<[f32; 3]> {
    let gain = amount.clamp(0.0, 1.0) * 1.35;
    let mut out = vec![[0.0f32; 3]; w * h];
    out.par_chunks_exact_mut(w)
        .enumerate()
        .for_each(|(y, dst_row)| {
            let ym = y.saturating_sub(1);
            let yp = (y + 1).min(h - 1);
            let rm = ym * w;
            let r0 = y * w;
            let rp = yp * w;
            for (x, dst_px) in dst_row.iter_mut().enumerate() {
                let xm = x.saturating_sub(1);
                let xp = (x + 1).min(w - 1);
                let c = src[r0 + x];
                let p00 = src[rm + xm];
                let p01 = src[rm + x];
                let p02 = src[rm + xp];
                let p10 = src[r0 + xm];
                let p12 = src[r0 + xp];
                let p20 = src[rp + xm];
                let p21 = src[rp + x];
                let p22 = src[rp + xp];
                let mut res = [0.0f32; 3];
                for ch in 0..3 {
                    let blur = (p00[ch]
                        + 2.0 * p01[ch]
                        + p02[ch]
                        + 2.0 * p10[ch]
                        + 4.0 * c[ch]
                        + 2.0 * p12[ch]
                        + p20[ch]
                        + 2.0 * p21[ch]
                        + p22[ch])
                        * (1.0 / 16.0);
                    res[ch] = (c[ch] + (c[ch] - blur) * gain).clamp(0.0, 1.0);
                }
                *dst_px = res;
            }
        });
    out
}

/// Returns a crisp 5×7 bitmap glyph (7 rows of 5 bits, MSB on left).
fn bitmap_glyph_5x7(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1C, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1C],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x06, 0x08, 0x10, 0x1F],
        '3' => [0x1E, 0x01, 0x01, 0x0E, 0x01, 0x01, 0x1E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x0E, 0x10, 0x1E, 0x11, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
        ',' => [0x00, 0x00, 0x00, 0x00, 0x0C, 0x04, 0x08],
        ':' => [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        '_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F],
        '/' => [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        '@' => [0x0E, 0x11, 0x17, 0x15, 0x17, 0x10, 0x0E],
        '&' => [0x0C, 0x12, 0x14, 0x08, 0x15, 0x12, 0x0D],
        '!' => [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04],
        '\'' => [0x04, 0x04, 0x08, 0x00, 0x00, 0x00, 0x00],
        '+' => [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00],
        '©' => [0x0E, 0x11, 0x16, 0x14, 0x16, 0x11, 0x0E],
        ' ' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        _ => [0x0E, 0x11, 0x01, 0x06, 0x04, 0x00, 0x04],
    }
}

/// Composites an anti-aliased studio text watermark (with subtle drop shadow for legibility on
/// both bright skies and deep shadows) onto an encoded sRGB/Display P3 `[0.0, 1.0]` buffer.
fn composite_text_watermark_srgb(
    buf: &mut [[f32; 3]],
    width: usize,
    height: usize,
    watermark: &TextWatermark,
) {
    let wm = watermark.sanitized();
    if !wm.is_active() || width == 0 || height == 0 {
        return;
    }
    let chars: Vec<char> = wm.text.chars().collect();
    if chars.is_empty() {
        return;
    }

    let glyph_w = 5usize;
    let glyph_h = 7usize;
    let glyph_advance = 6usize;
    let mask_w = chars.len() * glyph_advance - 1;
    let mask_h = glyph_h;

    let mut mask = vec![0.0_f32; mask_w * mask_h];
    for (ci, &ch) in chars.iter().enumerate() {
        let rows = bitmap_glyph_5x7(ch);
        let x_off = ci * glyph_advance;
        for (ry, &bits) in rows.iter().enumerate() {
            for rx in 0..glyph_w {
                if (bits & (1u8 << (4 - rx))) != 0 {
                    mask[ry * mask_w + x_off + rx] = 1.0;
                }
            }
        }
    }

    let min_dim = width.min(height) as f32;
    let max_text_w = (width as f32 * 0.86).max(1.0);
    let max_text_h = (height as f32 * 0.35).max(1.0);
    let raw_cell = (min_dim / 220.0).clamp(1.0, 28.0) * wm.scale;
    let cell = raw_cell
        .min(max_text_w / mask_w as f32)
        .min(max_text_h / mask_h as f32)
        .max(0.5);

    let text_w = (mask_w as f32 * cell).ceil() as usize;
    let text_h = (mask_h as f32 * cell).ceil() as usize;
    let max_mx = width / 4;
    let max_my = height / 4;
    let margin_x = ((width as f32 * 0.035).round() as usize).clamp(2.min(max_mx), max_mx);
    let margin_y = ((height as f32 * 0.035).round() as usize).clamp(2.min(max_my), max_my);

    let (origin_x, origin_y) = match wm.position {
        WatermarkPosition::BottomRight => (
            width.saturating_sub(text_w + margin_x),
            height.saturating_sub(text_h + margin_y),
        ),
        WatermarkPosition::BottomLeft => (margin_x, height.saturating_sub(text_h + margin_y)),
        WatermarkPosition::TopRight => (width.saturating_sub(text_w + margin_x), margin_y),
        WatermarkPosition::TopLeft => (margin_x, margin_y),
        WatermarkPosition::Center => (
            width.saturating_sub(text_w) / 2,
            height.saturating_sub(text_h) / 2,
        ),
    };

    let sample_mask = |mx: f32, my: f32| -> f32 {
        if mx < -0.5 || my < -0.5 || mx > mask_w as f32 - 0.5 || my > mask_h as f32 - 0.5 {
            return 0.0;
        }
        let x0 = mx.floor() as isize;
        let y0 = my.floor() as isize;
        let tx = mx - x0 as f32;
        let ty = my - y0 as f32;
        let get = |ix: isize, iy: isize| -> f32 {
            if ix < 0 || iy < 0 || ix >= mask_w as isize || iy >= mask_h as isize {
                0.0
            } else {
                mask[iy as usize * mask_w + ix as usize]
            }
        };
        let a = lerp_f32(get(x0, y0), get(x0 + 1, y0), tx);
        let b = lerp_f32(get(x0, y0 + 1), get(x0 + 1, y0 + 1), tx);
        lerp_f32(a, b, ty)
    };

    let shadow_off = (cell * 0.55).max(1.0);
    let pad = shadow_off.ceil() as usize + 1;
    let x_end = (origin_x + text_w + pad).min(width);
    let y_end = (origin_y + text_h + pad).min(height);
    let fg = [0.97_f32, 0.97, 0.98];
    let sh = [0.04_f32, 0.04, 0.05];

    for py in origin_y..y_end {
        let row_base = py * width;
        let fy = (py - origin_y) as f32;
        for px in origin_x..x_end {
            let fx = (px - origin_x) as f32;
            let mx = (fx + 0.5) / cell - 0.5;
            let my = (fy + 0.5) / cell - 0.5;
            let fg_cov = sample_mask(mx, my);
            let sh_cov = sample_mask(
                (fx - shadow_off + 0.5) / cell - 0.5,
                (fy - shadow_off + 0.5) / cell - 0.5,
            );
            if fg_cov <= 1e-4 && sh_cov <= 1e-4 {
                continue;
            }
            let mut dst = buf[row_base + px];
            let sh_a = (sh_cov * wm.opacity * 0.55).clamp(0.0, 1.0);
            if sh_a > 1e-4 {
                for ch in 0..3 {
                    dst[ch] = lerp_f32(dst[ch], sh[ch], sh_a);
                }
            }
            let fg_a = (fg_cov * wm.opacity).clamp(0.0, 1.0);
            if fg_a > 1e-4 {
                for ch in 0..3 {
                    dst[ch] = lerp_f32(dst[ch], fg[ch], fg_a);
                }
            }
            buf[row_base + px] = dst;
        }
    }
}

/// Fused single-pass post-resize 3×3 Gaussian unsharp mask and quantization in sRGB space.
///
/// Reads directly from `src` and writes quantized channels into `out` without cloning `src`.
fn sharpen_and_quantize<T: Copy + Default + Send + Sync>(
    src: &[[f32; 3]],
    w: usize,
    h: usize,
    amount: f32,
    quantize: impl Fn(f32) -> T + Sync,
) -> Vec<T> {
    let gain = amount.clamp(0.0, 1.0) * 1.35;
    let mut out = vec![T::default(); w * h * 3];

    out.par_chunks_exact_mut(w * 3)
        .enumerate()
        .for_each(|(y, dst_row)| {
            let ym = y.saturating_sub(1);
            let yp = (y + 1).min(h - 1);
            let rm = ym * w;
            let r0 = y * w;
            let rp = yp * w;

            for x in 0..w {
                let xm = x.saturating_sub(1);
                let xp = (x + 1).min(w - 1);

                let c = src[r0 + x];
                let p00 = src[rm + xm];
                let p01 = src[rm + x];
                let p02 = src[rm + xp];
                let p10 = src[r0 + xm];
                let p12 = src[r0 + xp];
                let p20 = src[rp + xm];
                let p21 = src[rp + x];
                let p22 = src[rp + xp];

                let didx = x * 3;
                for ch in 0..3 {
                    let blur = (p00[ch]
                        + 2.0 * p01[ch]
                        + p02[ch]
                        + 2.0 * p10[ch]
                        + 4.0 * c[ch]
                        + 2.0 * p12[ch]
                        + p20[ch]
                        + 2.0 * p21[ch]
                        + p22[ch])
                        * (1.0 / 16.0);
                    let sharpened = (c[ch] + (c[ch] - blur) * gain).clamp(0.0, 1.0);
                    dst_row[didx + ch] = quantize(sharpened);
                }
            }
        });

    out
}

/// Evaluates a single linear RGB pixel using a precompiled [`CompiledRecipe`] in
/// scene-referred linear light, applies optical radial vignette and subtle
/// midtone-weighted film grain pre-tonemap (matching `shaders/develop.wgsl`),
/// then applies [`filmic_tonemap`] and encodes into `[0.0, 1.0]` sRGB.
#[allow(clippy::too_many_arguments)]
#[inline]
fn evaluate_export_pixel_srgb(
    src_rgb: [f32; 3],
    u: f32,
    v: f32,
    px_x: f32,
    px_y: f32,
    frame_meta: ExportFrameMeta,
    recipe: &DevelopRecipe,
    compiled: &CompiledRecipe,
    color_space: ExportColorSpace,
    layers: &[AdjustmentLayer],
    layer_atlas: Option<&LayerMaskAtlas>,
    local_mid_ratio: f32,
    local_hp_rel: f32,
) -> [f32; 3] {
    let mut rgb = compiled.evaluate_pixel_scene_linear(src_rgb);

    let ori_u = frame_meta.crop[0] + u * frame_meta.crop[2];
    let ori_v = frame_meta.crop[1] + v * frame_meta.crop[3];

    // Stage 10b: Adjustment Layers (matching shaders/develop.wgsl bit-for-bit).
    if let Some(atlas) = layer_atlas
        && !layers.is_empty()
    {
        let [img_u, img_v] = if frame_meta.geo_or_lens_active {
            let [su, sv] = if frame_meta.respect_crop {
                recipe.geometry.oriented_to_source_uv_unclamped(
                    ori_u,
                    ori_v,
                    frame_meta.src_size[0],
                    frame_meta.src_size[1],
                )
            } else {
                [ori_u, ori_v]
            };
            apply_lens_distortion_uv(
                [su, sv],
                recipe.lens.sanitized().distortion,
                frame_meta.src_size[0],
                frame_meta.src_size[1],
            )
        } else {
            [ori_u, ori_v]
        };

        for (li, layer) in layers.iter().enumerate() {
            if layer.is_active() {
                let luma_tone = luma709(rgb).max(0.0).sqrt().clamp(0.0, 1.0);
                let m = atlas.evaluate_layer_mask(
                    li,
                    img_u,
                    img_v,
                    luma_tone,
                    layer.inverted,
                    layer.opacity,
                );
                rgb = apply_local_adjustments_linear(
                    rgb,
                    &layer.adjustments,
                    m,
                    local_mid_ratio,
                    local_hp_rel,
                );
            }
        }
    }

    // Optical radial cos^4 light falloff compensation anchored to full lens center.
    let light_falloff = if recipe.lens.light_falloff.is_finite() {
        recipe.lens.light_falloff.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if light_falloff > 1e-5 {
        let lcx = (ori_u - 0.5) * 2.0;
        let lcy = (ori_v - 0.5) * 2.0;
        let lr2 = lcx * lcx + lcy * lcy;
        let gain = (1.0 + 0.22 * lr2).powf(2.0 * light_falloff);
        rgb[0] *= gain;
        rgb[1] *= gain;
        rgb[2] *= gain;
    }

    // Crop-aware optical vignette in scene-referred linear light (matches shaders/develop.wgsl).
    if recipe.vignette.abs() > 1e-5 {
        let (cx, cy) = match recipe.vignette_method {
            VignetteMethod::EllipticOnCrop => ((u - 0.5) * 2.0, (v - 0.5) * 2.0),
            VignetteMethod::CircularOnCrop => {
                let aspect = frame_meta.crop_size[0].max(1.0) / frame_meta.crop_size[1].max(1.0);
                let ax = aspect.sqrt();
                let ay = 1.0 / ax;
                ((u - 0.5) * 2.0 * ax, (v - 0.5) * 2.0 * ay)
            }
            VignetteMethod::Circular => {
                let aspect = frame_meta.ori_size[0].max(1.0) / frame_meta.ori_size[1].max(1.0);
                let ax = aspect.sqrt();
                let ay = 1.0 / ax;
                ((ori_u - 0.5) * 2.0 * ax, (ori_v - 0.5) * 2.0 * ay)
            }
        };
        let r2 = cx * cx + cy * cy;
        let vig_gain = 2.0_f32.powf(recipe.vignette * 1.15 * smoothstep(0.18, 1.65, r2));
        rgb[0] *= vig_gain;
        rgb[1] *= vig_gain;
        rgb[2] *= vig_gain;
    }

    // Photographic film grain pre-tonemap (6 emulsion profiles + Granularity; matches shaders/develop.wgsl).
    if recipe.grain > 0.001 {
        let grain_size = if recipe.grain_size.is_finite() {
            recipe.grain_size.clamp(0.0, 1.0)
        } else {
            0.35
        };
        rgb = apply_film_grain_cpu(rgb, px_x, px_y, recipe.grain, recipe.grain_type, grain_size);
    }

    let mapped = filmic_tonemap_with_curve(rgb, compiled.film_curve);
    let gamut = match color_space {
        ExportColorSpace::Srgb => mapped,
        ExportColorSpace::DisplayP3 => linear_srgb_to_display_p3(mapped),
    };
    [
        linear_to_srgb(gamut[0]),
        linear_to_srgb(gamut[1]),
        linear_to_srgb(gamut[2]),
    ]
}

#[inline]
fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[inline]
fn value_noise_2d(px: f32, py: f32) -> f32 {
    let ix = px.floor();
    let iy = py.floor();
    let fx = px - ix;
    let fy = py - iy;
    let wx = fx * fx * (3.0 - 2.0 * fx);
    let wy = fy * fy * (3.0 - 2.0 * fy);
    let a = hash12(ix, iy);
    let b = hash12(ix + 1.0, iy);
    let c = hash12(ix, iy + 1.0);
    let d = hash12(ix + 1.0, iy + 1.0);
    lerp_f32(lerp_f32(a, b, wx), lerp_f32(c, d, wx), wy)
}

#[inline]
fn apply_film_grain_cpu(
    rgb: [f32; 3],
    px_x: f32,
    px_y: f32,
    grain: f32,
    grain_type: GrainType,
    grain_size: f32,
) -> [f32; 3] {
    let sz = grain_size.clamp(0.0, 1.0);
    let luma = (0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]).max(1e-5);
    let t = (luma / (luma + 0.18)).clamp(0.0, 1.0);
    let mid_mask = 1.0 - (2.0 * t - 1.0).abs();
    let base_n = (hash12(px_x, px_y) - 0.5) * 2.0;

    let (n, amp, tone_w) = match grain_type {
        GrainType::Fine => {
            let cell = 1.0 + sz * 2.5;
            let c_n =
                (hash12((px_x / cell).floor() + 17.0, (px_y / cell).floor() + 31.0) - 0.5) * 2.0;
            (
                lerp_f32(base_n, c_n, sz * 0.45),
                0.065,
                0.35 + 0.65 * mid_mask,
            )
        }
        GrainType::SilverRich => {
            let cell = 1.0 + sz * 2.5;
            let c_n =
                (hash12((px_x / cell).floor() + 17.0, (px_y / cell).floor() + 31.0) - 0.5) * 2.0;
            let m = lerp_f32(base_n, c_n, 0.35 + sz * 0.50);
            let sgn = if m > 0.0 {
                1.0
            } else if m < 0.0 {
                -1.0
            } else {
                0.0
            };
            let shaped = sgn * m.abs().max(1e-6).powf(0.85);
            (shaped, 0.085, 0.45 + 0.55 * mid_mask)
        }
        GrainType::Soft => {
            let cell = 1.2 + sz * 2.8;
            let sn = (value_noise_2d(px_x / cell + 5.0, px_y / cell + 11.0) - 0.5) * 2.0;
            (lerp_f32(sn, base_n, 0.25), 0.060, 0.30 + 0.70 * mid_mask)
        }
        GrainType::Cubic => {
            let cell = 1.25 + sz * 2.75;
            let cn =
                (hash12((px_x / cell).floor() + 41.0, (px_y / cell).floor() + 73.0) - 0.5) * 2.0;
            (
                lerp_f32(base_n, cn, 0.55 + sz * 0.35),
                0.095,
                0.40 + 0.60 * mid_mask,
            )
        }
        GrainType::Tabular => {
            let cell = 1.0 + sz * 1.6;
            let tn = (value_noise_2d(px_x / cell + 23.0, px_y / cell + 19.0) - 0.5) * 2.0;
            (
                lerp_f32(base_n, tn, 0.30 + sz * 0.35),
                0.070,
                0.25 + 0.75 * mid_mask,
            )
        }
        GrainType::Harsh => {
            let cell = 1.4 + sz * 3.1;
            let hn =
                (hash12((px_x / cell).floor() + 97.0, (px_y / cell).floor() + 53.0) - 0.5) * 2.0;
            let shaped = (lerp_f32(base_n, hn, 0.60 + sz * 0.35) * 1.35).clamp(-1.0, 1.0);
            (shaped, 0.125, 0.55 + 0.45 * mid_mask)
        }
    };

    let delta = n * grain * amp * tone_w;
    [
        (rgb[0] + delta).max(0.0),
        (rgb[1] + delta).max(0.0),
        (rgb[2] + delta).max(0.0),
    ]
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn fract(x: f32) -> f32 {
    x - x.floor()
}

/// Deterministic spatial noise hash matching `hash12` in `shaders/develop.wgsl`.
#[inline]
fn hash12(px: f32, py: f32) -> f32 {
    let mut p3x = fract(px * 0.1031);
    let mut p3y = fract(py * 0.1031);
    let mut p3z = fract(px * 0.1031);
    let d = p3x * (p3y + 33.33) + p3y * (p3z + 33.33) + p3z * (p3x + 33.33);
    p3x += d;
    p3y += d;
    p3z += d;
    fract((p3x + p3y) * p3z)
}
