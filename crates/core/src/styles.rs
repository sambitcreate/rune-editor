//! User Styles persistence (`~/.config/rune-darkroom/styles/<slug>.json`) and Capture One-style
//! additive Style Stacking.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    ADVANCED_COLOR_SLICE_COUNT, CoreError, DevelopRecipe, HSL_BAND_COUNT,
    patch::{RecipePatch, blend_recipes, recipe_to_sparse_patch},
    registry::{SCALAR_PARAMS, copy_recipe_groups},
    session::atomic_write_bytes,
};

/// Default non-geometry groups captured when saving a User Style with an empty `groups` slice.
const DEFAULT_STYLE_GROUPS: &[&str] = &[
    "wb",
    "tone",
    "levels",
    "curves",
    "wheels",
    "color",
    "skin_tone",
    "bw",
    "detail",
    "effects",
    "lens",
];

/// A user-created photographic Style persisted as clean, sparse JSON.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserStyle {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub patch: RecipePatch,
}

impl UserStyle {
    /// Captures a sparse [`UserStyle`] from `recipe`, restricted to `groups` (or all non-geometry
    /// photographic tool groups when `groups` is empty).
    pub fn from_recipe(
        name: impl Into<String>,
        description: impl Into<String>,
        recipe: &DevelopRecipe,
        groups: &[String],
    ) -> Result<Self, CoreError> {
        let name = name.into();
        // Validate slug upfront so invalid/traversal style names fail immediately.
        let _ = sanitize_style_slug(&name)?;

        let effective_groups: Vec<String> = if groups.is_empty() {
            DEFAULT_STYLE_GROUPS
                .iter()
                .map(|g| (*g).to_owned())
                .collect()
        } else {
            groups.to_vec()
        };

        let mut filtered = DevelopRecipe::default();
        copy_recipe_groups(&mut filtered, recipe, &effective_groups)?;
        let patch = recipe_to_sparse_patch(&filtered);

        Ok(Self {
            name: name.trim().to_owned(),
            description: description.into(),
            groups: effective_groups,
            patch,
        })
    }

    /// Returns the filesystem-safe slug for this style's name.
    pub fn slug(&self) -> Result<String, CoreError> {
        sanitize_style_slug(&self.name)
    }
}

/// Validates and converts a user style name into a safe `[a-z0-9_-]` filename stem.
///
/// Rejects empty strings, path separators (`/`, `\`), parent traversal (`..`), NUL bytes,
/// and leading dots to guarantee zero path traversal under the styles directory.
pub fn sanitize_style_slug(name: &str) -> Result<String, CoreError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(CoreError::Validation(
            "Style name cannot be empty".to_owned(),
        ));
    }
    if trimmed.contains("..")
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains('\0')
        || trimmed.starts_with('.')
    {
        return Err(CoreError::Validation(format!(
            "Invalid style name '{name}': path separators, leading dots, and '..' are forbidden"
        )));
    }

    let mut slug = String::with_capacity(trimmed.len().min(64));
    let mut prev_sep = false;
    for ch in trimmed.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            slug.push(lower);
            prev_sep = false;
        } else if (lower == '-' || lower == '_' || lower.is_ascii_whitespace())
            && !prev_sep
            && !slug.is_empty()
        {
            slug.push(if lower == '-' { '-' } else { '_' });
            prev_sep = true;
        }
        if slug.len() >= 64 {
            break;
        }
    }
    while slug.ends_with('_') || slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        return Err(CoreError::Validation(format!(
            "Style name '{name}' contains no valid alphanumeric characters"
        )));
    }
    Ok(slug)
}

/// Returns the default user styles directory (`~/.config/rune-darkroom/styles`).
pub fn default_styles_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("rune-darkroom").join("styles");
    }
    if let Some(home) = std::env::var_os("HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home)
            .join(".config")
            .join("rune-darkroom")
            .join("styles");
    }
    PathBuf::from(".rune-darkroom-styles")
}

fn style_path_in_dir(dir: &Path, name_or_slug: &str) -> Result<PathBuf, CoreError> {
    let slug = sanitize_style_slug(name_or_slug)?;
    let filename = format!("{slug}.json");
    let path = dir.join(&filename);
    if path.parent() != Some(dir) {
        return Err(CoreError::Validation(format!(
            "Refusing style path outside styles directory: '{name_or_slug}'"
        )));
    }
    Ok(path)
}

/// Atomically saves `style` under `~/.config/rune-darkroom/styles/<slug>.json`.
pub fn save_user_style(style: &UserStyle) -> Result<PathBuf, CoreError> {
    save_user_style_in_dir(&default_styles_dir(), style)
}

/// Atomically saves `style` under `dir/<slug>.json` via temp-dotfile + `sync_all` + `rename`.
pub fn save_user_style_in_dir(dir: &Path, style: &UserStyle) -> Result<PathBuf, CoreError> {
    let path = style_path_in_dir(dir, &style.name)?;
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(style)?;
    atomic_write_bytes(&path, json.as_bytes())?;
    Ok(path)
}

/// Lists all valid user styles saved in `~/.config/rune-darkroom/styles`.
pub fn list_user_styles() -> Result<Vec<UserStyle>, CoreError> {
    list_user_styles_in_dir(&default_styles_dir())
}

/// Lists all valid user styles saved in `dir`, sorted by name case-insensitively.
pub fn list_user_styles_in_dir(dir: &Path) -> Result<Vec<UserStyle>, CoreError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut styles = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(fname) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if fname.starts_with('.') || !fname.ends_with(".json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(style) = serde_json::from_str::<UserStyle>(&text) {
            styles.push(style);
        }
    }
    styles.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(styles)
}

/// Loads a user style by display name or slug from `~/.config/rune-darkroom/styles`.
pub fn load_user_style(name_or_slug: &str) -> Result<UserStyle, CoreError> {
    load_user_style_in_dir(&default_styles_dir(), name_or_slug)
}

/// Loads a user style by display name or slug from `dir`.
pub fn load_user_style_in_dir(dir: &Path, name_or_slug: &str) -> Result<UserStyle, CoreError> {
    let path = style_path_in_dir(dir, name_or_slug)?;
    if !path.exists() {
        return Err(CoreError::UnknownPreset(name_or_slug.to_owned()));
    }
    let text = std::fs::read_to_string(&path)?;
    let style: UserStyle = serde_json::from_str(&text)?;
    Ok(style)
}

/// Deletes a user style by display name or slug from `~/.config/rune-darkroom/styles`.
pub fn delete_user_style(name_or_slug: &str) -> Result<bool, CoreError> {
    delete_user_style_in_dir(&default_styles_dir(), name_or_slug)
}

/// Deletes a user style by display name or slug from `dir`. Returns `Ok(true)` if removed.
pub fn delete_user_style_in_dir(dir: &Path, name_or_slug: &str) -> Result<bool, CoreError> {
    let path = style_path_in_dir(dir, name_or_slug)?;
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&path)?;
    Ok(true)
}

/// Applies `style` in replace mode onto `target` at `strength` in `0.0..=1.0`, preserving
/// `target.geometry` unless the style explicitly includes geometry.
pub fn apply_style_to_recipe(
    target: &mut DevelopRecipe,
    style: &UserStyle,
    strength: f32,
) -> Result<(), CoreError> {
    let s = if strength.is_finite() {
        strength.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let saved_geometry = target.geometry;
    let includes_geo = style.patch.geometry.is_some()
        && style.groups.iter().any(|g| {
            let gl = g.trim().to_ascii_lowercase();
            gl == "geometry" || gl == "crop" || gl == "all"
        });

    let mut replaced = DevelopRecipe::default();
    if !includes_geo {
        replaced.geometry = saved_geometry;
    }
    style.patch.apply_to(&mut replaced, None)?;
    if !includes_geo {
        replaced.geometry = saved_geometry;
    }

    let mut blended = blend_recipes(target, &replaced, s);
    blended.geometry = if includes_geo && s >= 0.5 {
        replaced.geometry
    } else {
        saved_geometry
    };
    if (s - 1.0).abs() <= 1e-5 {
        blended.preset_name = Some(style.name.clone());
    }
    *target = blended.sanitized();
    Ok(())
}

/// Capture One-style cumulative Style Stacking: applies `style_patch` additively/blended
/// at `strength` (`0.0..=1.0`) onto `target` while preserving untouched adjustments and
/// ALWAYS preserving `target.geometry` (crop, straighten, rotation, flip, keystone).
pub fn stack_style_onto_recipe(
    target: &mut DevelopRecipe,
    style_patch: &RecipePatch,
    strength: f32,
) -> Result<(), CoreError> {
    let s = if strength.is_finite() {
        strength.clamp(0.0, 1.0)
    } else {
        1.0
    };
    if s <= 1e-5 {
        return Ok(());
    }

    let saved_geometry = target.geometry;
    let mut patch_no_geo = style_patch.clone();
    patch_no_geo.geometry = None;

    // Evaluate style's standalone delta from Neutral so additive parameters stack cleanly.
    let neutral = DevelopRecipe::default();
    let mut style_recipe = neutral.clone();
    patch_no_geo.apply_to(&mut style_recipe, None)?;

    // Also evaluate patch directly onto a clone of target for structured/enum blending.
    let mut direct_patched = target.clone();
    patch_no_geo.apply_to(&mut direct_patched, None)?;

    let mut out = target.clone();

    // 1. Additive stacking for top-level scalar sliders relative to their neutral defaults.
    for spec in SCALAR_PARAMS {
        let style_v = (spec.get)(&style_recipe);
        let delta = style_v - spec.default;
        if delta.abs() > 1e-6 {
            let cur = (spec.get)(target);
            (spec.set)(&mut out, (cur + delta * s).clamp(spec.min, spec.max));
        }
    }

    // 2. Additive stacking for 4 Color Balance wheels.
    let stack_wheel = |cur: crate::ColorWheel, st: crate::ColorWheel| {
        if st.is_neutral() {
            cur
        } else {
            crate::ColorWheel::new(
                cur.hue_x + st.hue_x * s,
                cur.hue_y + st.hue_y * s,
                cur.luma + st.luma * s,
            )
            .clamped()
        }
    };
    out.shadows_wheel = stack_wheel(target.shadows_wheel, style_recipe.shadows_wheel);
    out.midtones_wheel = stack_wheel(target.midtones_wheel, style_recipe.midtones_wheel);
    out.highlights_wheel = stack_wheel(target.highlights_wheel, style_recipe.highlights_wheel);
    out.master_wheel = stack_wheel(target.master_wheel, style_recipe.master_wheel);

    // 3. Additive stacking for 8-band HSL.
    for i in 0..HSL_BAND_COUNT {
        let st = style_recipe.hsl[i];
        if !st.is_neutral() {
            out.hsl[i] = crate::HslBand::new(
                (target.hsl[i].hue + st.hue * s).clamp(-1.0, 1.0),
                (target.hsl[i].saturation + st.saturation * s).clamp(-1.0, 1.0),
                (target.hsl[i].luminance + st.luminance * s).clamp(-1.0, 1.0),
            );
        }
    }

    // 4. Blended application for structured/discrete tool blocks ONLY when present in style_patch.
    let lerp = |a: f32, b: f32| a + (b - a) * s;

    if patch_no_geo.processing_mode.is_some() && s >= 0.5 {
        out.processing_mode = direct_patched.processing_mode;
    }
    if patch_no_geo.film_negative.is_some() {
        out.film_negative = crate::FilmNegativeParams {
            base_r: lerp(
                target.film_negative.base_r,
                direct_patched.film_negative.base_r,
            ),
            base_g: lerp(
                target.film_negative.base_g,
                direct_patched.film_negative.base_g,
            ),
            base_b: lerp(
                target.film_negative.base_b,
                direct_patched.film_negative.base_b,
            ),
            density: lerp(
                target.film_negative.density,
                direct_patched.film_negative.density,
            ),
            contrast: lerp(
                target.film_negative.contrast,
                direct_patched.film_negative.contrast,
            ),
            black_clip: lerp(
                target.film_negative.black_clip,
                direct_patched.film_negative.black_clip,
            ),
            white_clip: lerp(
                target.film_negative.white_clip,
                direct_patched.film_negative.white_clip,
            ),
        }
        .sanitized();
    }
    if patch_no_geo.camera_profile.is_some() && s >= 0.5 {
        out.camera_profile = direct_patched.camera_profile.clone();
    }
    if patch_no_geo.film_curve.is_some() && s >= 0.5 {
        out.film_curve = direct_patched.film_curve;
    }
    if patch_no_geo.wb_preset.is_some() && s >= 0.5 {
        out.wb_preset = direct_patched.wb_preset;
    } else if patch_no_geo.temperature.is_some() || patch_no_geo.tint.is_some() {
        out.wb_preset = crate::WbPreset::Custom;
    }
    if patch_no_geo.clarity_method.is_some() && s >= 0.5 {
        out.clarity_method = direct_patched.clarity_method;
    }
    if patch_no_geo.grain_type.is_some() && s >= 0.5 {
        out.grain_type = direct_patched.grain_type;
    }
    if patch_no_geo.vignette_method.is_some() && s >= 0.5 {
        out.vignette_method = direct_patched.vignette_method;
    }
    if patch_no_geo.curves.is_some() && s >= 0.5 {
        out.curves = direct_patched.curves.clone();
    }
    if patch_no_geo.levels.is_some() {
        let lerp_ch = |a: crate::ChannelLevels, b: crate::ChannelLevels| crate::ChannelLevels {
            in_black: lerp(a.in_black, b.in_black),
            in_mid: lerp(a.in_mid, b.in_mid),
            in_white: lerp(a.in_white, b.in_white),
            out_black: lerp(a.out_black, b.out_black),
            out_white: lerp(a.out_white, b.out_white),
        };
        out.levels = crate::LevelsParams {
            in_black: lerp(target.levels.in_black, direct_patched.levels.in_black),
            in_mid: lerp(target.levels.in_mid, direct_patched.levels.in_mid),
            in_white: lerp(target.levels.in_white, direct_patched.levels.in_white),
            out_black: lerp(target.levels.out_black, direct_patched.levels.out_black),
            out_white: lerp(target.levels.out_white, direct_patched.levels.out_white),
            red: lerp_ch(target.levels.red, direct_patched.levels.red),
            green: lerp_ch(target.levels.green, direct_patched.levels.green),
            blue: lerp_ch(target.levels.blue, direct_patched.levels.blue),
        }
        .sanitized();
    }
    if patch_no_geo.bw.is_some() {
        if s >= 0.5 {
            out.bw.enabled = direct_patched.bw.enabled;
        }
        out.bw.red = lerp(target.bw.red, direct_patched.bw.red);
        out.bw.yellow = lerp(target.bw.yellow, direct_patched.bw.yellow);
        out.bw.green = lerp(target.bw.green, direct_patched.bw.green);
        out.bw.cyan = lerp(target.bw.cyan, direct_patched.bw.cyan);
        out.bw.blue = lerp(target.bw.blue, direct_patched.bw.blue);
        out.bw.magenta = lerp(target.bw.magenta, direct_patched.bw.magenta);
        out.bw.split_shadow_hue = lerp(
            target.bw.split_shadow_hue,
            direct_patched.bw.split_shadow_hue,
        );
        out.bw.split_shadow_sat = lerp(
            target.bw.split_shadow_sat,
            direct_patched.bw.split_shadow_sat,
        );
        out.bw.split_highlight_hue = lerp(
            target.bw.split_highlight_hue,
            direct_patched.bw.split_highlight_hue,
        );
        out.bw.split_highlight_sat = lerp(
            target.bw.split_highlight_sat,
            direct_patched.bw.split_highlight_sat,
        );
        out.bw = out.bw.sanitized();
    }
    if patch_no_geo.advanced_color.is_some() {
        for i in 0..ADVANCED_COLOR_SLICE_COUNT {
            let a = target.advanced_color[i];
            let b = direct_patched.advanced_color[i];
            if a != b {
                out.advanced_color[i] = crate::ColorEditorSlice {
                    enabled: if s >= 0.5 { b.enabled } else { a.enabled },
                    target_hue: if s >= 0.5 { b.target_hue } else { a.target_hue },
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
        }
    }
    if patch_no_geo.skin_tone.is_some() {
        let a = target.skin_tone;
        let b = direct_patched.skin_tone;
        out.skin_tone = crate::SkinToneParams {
            enabled: if s >= 0.5 { b.enabled } else { a.enabled },
            target_hue: if s >= 0.5 { b.target_hue } else { a.target_hue },
            target_sat: lerp(a.target_sat, b.target_sat),
            target_lightness: lerp(a.target_lightness, b.target_lightness),
            smoothness: lerp(a.smoothness, b.smoothness),
            uniformity_hue: lerp(a.uniformity_hue, b.uniformity_hue),
            uniformity_sat: lerp(a.uniformity_sat, b.uniformity_sat),
            uniformity_lightness: lerp(a.uniformity_lightness, b.uniformity_lightness),
            amount_hue: lerp(a.amount_hue, b.amount_hue),
            amount_sat: lerp(a.amount_sat, b.amount_sat),
            amount_lightness: lerp(a.amount_lightness, b.amount_lightness),
        }
        .sanitized();
    }
    if patch_no_geo.lens.is_some() {
        out.lens = crate::LensParams {
            distortion: lerp(target.lens.distortion, direct_patched.lens.distortion),
            chromatic_aberration: lerp(
                target.lens.chromatic_aberration,
                direct_patched.lens.chromatic_aberration,
            ),
            purple_fringing: lerp(
                target.lens.purple_fringing,
                direct_patched.lens.purple_fringing,
            ),
            light_falloff: lerp(target.lens.light_falloff, direct_patched.lens.light_falloff),
        }
        .sanitized();
    }

    out.geometry = saved_geometry;
    out.preset_name = None;
    *target = out.sanitized();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ColorWheel, GrainType, ToneCurves};

    #[test]
    fn style_slug_sanitizes_valid_names_and_rejects_traversal() {
        assert_eq!(
            sanitize_style_slug("Kodak Portra 400").unwrap(),
            "kodak_portra_400"
        );
        assert_eq!(
            sanitize_style_slug("  Warm Editorial - v2  ").unwrap(),
            "warm_editorial_v2"
        );
        for bad in [
            "",
            "   ",
            "../evil",
            "..\\evil",
            "foo/bar",
            "foo\\bar",
            ".hidden",
            "a..b",
            "bad\0name",
            "!!!",
        ] {
            assert!(
                sanitize_style_slug(bad).is_err(),
                "expected traversal/invalid style name '{bad}' to be rejected"
            );
        }
    }

    #[test]
    fn user_style_roundtrip_and_stacking_preserves_geometry() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "rune_core_styles_test_{}_{nanos}",
            std::process::id()
        ));

        let look_a = DevelopRecipe {
            exposure: 0.4,
            contrast: 0.2,
            temperature: 0.25,
            shadows_wheel: ColorWheel::new(-0.1, -0.2, 0.0),
            curves: ToneCurves {
                luma: vec![[0.0, 0.02], [0.5, 0.55], [1.0, 0.98]],
                ..ToneCurves::default()
            },
            ..DevelopRecipe::default()
        };
        let style_a = UserStyle::from_recipe(
            "Warm Portrait",
            "Warm midtones with cool shadow grade",
            &look_a,
            &[],
        )
        .expect("build style_a");

        let saved_path = save_user_style_in_dir(&dir, &style_a).expect("save style_a");
        assert!(saved_path.exists());
        assert_eq!(
            saved_path.file_name().and_then(|s| s.to_str()),
            Some("warm_portrait.json")
        );

        let listed = list_user_styles_in_dir(&dir).expect("list styles");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], style_a);

        let loaded = load_user_style_in_dir(&dir, "warm_portrait").expect("load by slug");
        assert_eq!(loaded, style_a);
        assert!(load_user_style_in_dir(&dir, "../escape").is_err());

        // Stack a second style (grain + vignette + extra contrast) onto a cropped target.
        let look_b = DevelopRecipe {
            contrast: 0.2,
            vignette: -0.4,
            grain: 0.3,
            grain_type: GrainType::SilverRich,
            ..DevelopRecipe::default()
        };
        let style_b = UserStyle::from_recipe("Silver Grain", "", &look_b, &[]).unwrap();

        let mut target = DevelopRecipe::default();
        target.geometry.crop = [0.1, 0.1, 0.8, 0.8];
        target.geometry.straighten_deg = 4.5;
        let saved_geo = target.geometry;

        apply_style_to_recipe(&mut target, &loaded, 1.0).expect("apply style_a");
        assert!((target.exposure - 0.4).abs() < 1e-5);
        assert_eq!(target.geometry, saved_geo);

        stack_style_onto_recipe(&mut target, &style_b.patch, 0.5).expect("stack style_b at 50%");
        assert_eq!(target.geometry, saved_geo);
        assert!(
            (target.exposure - 0.4).abs() < 1e-5,
            "untouched exposure preserved"
        );
        assert!(
            (target.contrast - 0.3).abs() < 1e-5,
            "contrast stacked additively"
        );
        assert!(
            (target.vignette + 0.2).abs() < 1e-5,
            "vignette scaled by strength"
        );
        assert!(
            (target.grain - 0.15).abs() < 1e-5,
            "grain scaled by strength"
        );
        assert_eq!(target.grain_type, GrainType::SilverRich);

        assert!(delete_user_style_in_dir(&dir, "Warm Portrait").unwrap());
        assert!(!delete_user_style_in_dir(&dir, "Warm Portrait").unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
