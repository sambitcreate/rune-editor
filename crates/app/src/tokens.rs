//! Centralized Anomaly OpenCode design tokens for Rune Darkroom.
//!
//! All UI modules (`main.rs`, `toolbar.rs`, `widgets.rs`, `filmstrip.rs`,
//! `color_wheel_tex.rs`) pull surfaces, hairlines, radii, typography, spacing,
//! and semantic scope/diagnostic colors exclusively from this module.

use rune::color::srgb_decode;
use rune::{Color, TextStyle};
use rune_kit::Kit;

/// Returns the canonical Anomaly OpenCode dark `Kit`.
#[inline]
pub fn kit() -> &'static Kit {
    rune_theme_opencode::kit(true)
}

/// Converts sRGB `(r, g, b)` components (`0.0..=1.0`) to a linear `rune::Color`.
#[inline]
pub fn srgb(r: f32, g: f32, b: f32) -> Color {
    Color {
        r: srgb_decode(r),
        g: srgb_decode(g),
        b: srgb_decode(b),
        a: 1.0,
    }
}

/// Spacing & padding scale (points).
pub mod space {
    pub const MICRO: f32 = 1.0;
    pub const XXS: f32 = 2.0;
    pub const TINY: f32 = 3.0;
    pub const XS: f32 = 4.0;
    pub const TIGHT: f32 = 5.0;
    pub const SM: f32 = 6.0;
    pub const CONTROL: f32 = 7.0;
    pub const MD: f32 = 8.0;
    pub const LG: f32 = 10.0;
    pub const XL: f32 = 12.0;
    pub const XXL: f32 = 14.0;
    pub const HUD_Y: f32 = 16.0;
    pub const HUD_X: f32 = 18.0;
    pub const HUD_BELOW_SPLIT: f32 = 48.0;
    pub const MODAL_PAD: f32 = 20.0;
}

/// Layout, widget, icon, and viewport dimensions (points).
pub mod size {
    pub const WINDOW_DEFAULT: (f32, f32) = (1360.0, 860.0);
    pub const WINDOW_MIN: (f32, f32) = (1024.0, 680.0);

    pub const TITLEBAR_H: f32 = 48.0;
    pub const STATUS_BAR_H: f32 = 28.0;
    pub const INSPECTOR_W: f32 = 356.0;
    pub const CARD_INNER_W: f32 = 308.0;

    pub const HISTOGRAM_W: f32 = 312.0;
    pub const HISTOGRAM_H: f32 = 64.0;
    pub const PROBE_FOOTER_H: f32 = 16.0;
    pub const LEVELS_STRIP_H: f32 = 38.0;
    pub const CURVE_H: f32 = 144.0;

    pub const FILMSTRIP_H: f32 = 160.0;
    pub const FILMSTRIP_HEADER_H: f32 = 34.0;
    pub const THUMB_CARD_W: f32 = 150.0;
    pub const THUMB_PREVIEW_W: f32 = 140.0;
    pub const THUMB_PREVIEW_H: f32 = 74.0;
    pub const THUMB_EMPTY_H: f32 = 92.0;
    pub const FLAG_BADGE: f32 = 16.0;

    pub const WHEEL_DIAMETER: f32 = 88.0;
    pub const MASTER_WHEEL_DIAMETER: f32 = 148.0;
    pub const WHEEL_PUCK: f32 = 11.0;
    pub const WHEEL_CROSSHAIR_ARM: f32 = 5.0;

    pub const SWATCH_RING: f32 = 26.0;
    pub const SWATCH_DOT: f32 = 14.0;
    pub const MODIFIED_DOT: f32 = 4.0;
    pub const CHANNEL_DOT: f32 = 6.0;
    pub const BAND_DOT: f32 = 8.0;
    pub const PROBE_SWATCH: f32 = 8.0;
    pub const TARGET_SWATCH: f32 = 14.0;
    pub const COLOR_TAG_DOT: f32 = 8.0;
    pub const COLOR_TAG_BTN: f32 = 16.0;
    pub const VARIANT_STRIP_H: f32 = 32.0;
    pub const PIN_MARKER: f32 = 18.0;
    pub const GRID_CARD_W: f32 = 216.0;
    pub const GRID_PREVIEW_H: f32 = 132.0;
    pub const EXPORT_CONTENT_MAX_W: f32 = 720.0;

    pub const PILL_COMPACT_H: f32 = 22.0;
    pub const PILL_H: f32 = 24.0;
    pub const ASPECT_PILL_H: f32 = 26.0;
    pub const ACTION_PILL_H: f32 = 28.0;
    pub const CAPSULE_SLIDER_H: f32 = 30.0;
    pub const MINI_CAPSULE_SLIDER_H: f32 = 22.0;
    pub const CAPSULE_ZERO_TICK_W: f32 = 1.0;
    pub const CAPSULE_EDGE_W: f32 = 1.0;
    pub const MINI_AUTO_BTN: f32 = 20.0;

    pub const CURVE_PT: f32 = 8.0;
    pub const CURVE_PT_DRAG: f32 = 10.0;
    pub const CURVE_PROBE_DOT: f32 = 6.0;

    pub const ICON_XS: f32 = 10.0;
    pub const ICON_SM: f32 = 11.0;
    pub const ICON_MD: f32 = 13.0;
    pub const ICON_LG: f32 = 15.0;

    pub const DIVIDER_TOOLBAR_H: f32 = 16.0;
    pub const DIVIDER_STATUS_H: f32 = 12.0;

    pub const DIALOG_ABOUT_W: f32 = 360.0;
    pub const DIALOG_CLOSE_W: f32 = 380.0;
    pub const SETTINGS_SIDEBAR_W: f32 = 220.0;
    pub const SETTINGS_CONTENT_MAX_W: f32 = 700.0;
    pub const STATUS_DOT: f32 = 7.0;

    pub const HUD_BLUR: f32 = 12.0;
    pub const HUD_BLUR_STRONG: f32 = 14.0;
    pub const SCROLL_FADE: f32 = 14.0;

    /// Phase 5 Capture One Proof Margin outer mat padding (points; adds to shader's ~14pt pad).
    pub const PROOF_MARGIN_STUDIO: f32 = 34.0;
    pub const PROOF_MARGIN_EXHIBITION: f32 = 70.0;
}

/// Border widths, focus rings, and vector stroke weights (points).
pub mod border {
    pub const NONE: f32 = 0.0;
    pub const HAIRLINE: f32 = 0.5;
    pub const THIN: f32 = 0.75;
    pub const REGULAR: f32 = 1.0;
    pub const CURVE_GHOST: f32 = 1.25;
    pub const ACTIVE: f32 = 1.5;
    pub const CURVE_ACTIVE: f32 = 1.75;
    pub const DROP_TARGET: f32 = 2.0;

    pub const FOCUS_RING: f32 = 2.0;
    pub const FOCUS_OFFSET_TIGHT: f32 = 1.0;
    pub const FOCUS_OFFSET: f32 = 2.0;

    pub const PUCK_SHADOW_Y: f32 = 1.0;
    pub const PUCK_SHADOW_BLUR: f32 = 3.0;
}

/// Radius helpers augmenting `k.radius.*`.
pub mod radius {
    pub const PROBE_SWATCH: f32 = 2.0;

    #[inline]
    pub const fn circle(diameter: f32) -> f32 {
        diameter * 0.5
    }
}

/// Named OpenCode typography tokens (including tabular monospace readouts).
pub mod ty {
    use super::*;

    fn mono_sized(k: &Kit, size: f32, line_height: f32) -> TextStyle {
        TextStyle {
            size,
            line_height,
            ..k.ty.mono
        }
        .features("'tnum' 1")
    }

    #[inline]
    pub fn mono_probe(k: &Kit) -> TextStyle {
        mono_sized(k, 9.5, 13.5)
    }

    #[inline]
    pub fn mono_micro(k: &Kit) -> TextStyle {
        mono_sized(k, 10.0, 14.0)
    }

    #[inline]
    pub fn mono_compact(k: &Kit) -> TextStyle {
        mono_sized(k, 10.5, 14.5)
    }

    #[inline]
    pub fn mono_value(k: &Kit) -> TextStyle {
        mono_sized(k, 11.0, 15.0)
    }

    #[inline]
    pub fn mono_status(k: &Kit) -> TextStyle {
        mono_sized(k, 11.0, 14.0)
    }

    #[inline]
    pub fn mono_hud(k: &Kit) -> TextStyle {
        mono_sized(k, 12.0, 16.0)
    }

    #[inline]
    pub fn section_header(k: &Kit) -> TextStyle {
        k.ty.small_medium
    }

    #[inline]
    pub fn control_label(k: &Kit) -> TextStyle {
        k.ty.text_xsmall
    }

    #[inline]
    pub fn control_label_medium(k: &Kit) -> TextStyle {
        k.ty.text_xsmall_medium
    }

    #[inline]
    pub fn brand_title(k: &Kit) -> TextStyle {
        k.ty.ui_medium
    }

    #[inline]
    pub fn caption(k: &Kit) -> TextStyle {
        k.ty.small
    }

    #[inline]
    pub fn dialog_title(k: &Kit) -> TextStyle {
        k.ty.body_medium
    }

    #[inline]
    pub fn dialog_body(k: &Kit) -> TextStyle {
        k.ty.text_small
    }
}

/// Semantic colors, translucent HUD surfaces, scope channels, and photographic swatches.
pub mod color {
    use super::*;

    pub const REJECT_CARD_OPACITY: f32 = 0.55;
    pub const HUD_ALPHA: f32 = 0.78;
    pub const HUD_STRONG_ALPHA: f32 = 0.86;
    pub const CURVE_GHOST_ALPHA: f32 = 0.32;

    #[inline]
    pub fn curve_ghost(base: Color) -> Color {
        base.alpha(CURVE_GHOST_ALPHA)
    }

    #[inline]
    pub fn hud_surface(k: &Kit) -> Color {
        k.c.bg_base.alpha(HUD_ALPHA)
    }

    #[inline]
    pub fn hud_surface_strong(k: &Kit) -> Color {
        k.c.bg_base.alpha(HUD_STRONG_ALPHA)
    }

    #[inline]
    pub fn pill_fill(k: &Kit, active: bool) -> Color {
        if active { k.c.layer_03 } else { k.c.bg_base }
    }

    #[inline]
    pub fn pill_border(k: &Kit, active: bool) -> Color {
        if active {
            k.c.border_focus
        } else {
            k.c.border_base
        }
    }

    // Scrubbable Capsule slider tokens (monochrome neutral OpenCode discipline)
    #[inline]
    pub fn capsule_bg(k: &Kit) -> Color {
        k.c.bg_base
    }

    #[inline]
    pub fn capsule_hover(k: &Kit) -> Color {
        k.c.layer_02
    }

    #[inline]
    pub fn capsule_fill(k: &Kit) -> Color {
        k.c.white.alpha(0.13)
    }

    #[inline]
    pub fn capsule_edge(k: &Kit) -> Color {
        k.c.white.alpha(0.24)
    }

    #[inline]
    pub fn capsule_zero_tick(k: &Kit) -> Color {
        k.c.white.alpha(0.10)
    }

    #[inline]
    pub fn curve_bg_hist(k: &Kit) -> Color {
        k.c.white.alpha(0.06)
    }

    #[inline]
    pub fn levels_hist_fill(k: &Kit) -> Color {
        k.c.white.alpha(0.16)
    }

    #[inline]
    pub fn levels_hist_stroke(k: &Kit) -> Color {
        k.c.white.alpha(0.38)
    }

    /// Capture One-style active tool modified indicator dot.
    #[inline]
    pub fn tool_modified_dot(k: &Kit) -> Color {
        k.c.text_accent
    }

    /// Capture One Pro warm-orange live cursor tonal indicator on the Tone Curve graph.
    #[inline]
    pub fn curve_tonal_indicator() -> Color {
        srgb(0.96, 0.56, 0.16)
    }

    #[inline]
    pub fn curve_tonal_indicator_line() -> Color {
        srgb(0.96, 0.56, 0.16).alpha(0.82)
    }

    /// Warm-amber bottom bar indicator on modified per-channel Levels & Color Editor tabs.
    #[inline]
    pub fn levels_modified_bar() -> Color {
        srgb(0.96, 0.56, 0.16)
    }

    /// Viewport diagnostic HUD accent when `View Selected Color Range` mask is active.
    #[inline]
    pub fn color_range_hud() -> Color {
        srgb(0.96, 0.56, 0.16)
    }

    /// Capture One Pro iconic warm-orange Compare Variant reference border (`#E87A2A`).
    #[inline]
    pub fn compare_reference_border() -> Color {
        srgb(0.91, 0.48, 0.16)
    }

    #[inline]
    pub fn compare_reference_bg() -> Color {
        srgb(0.91, 0.48, 0.16).alpha(0.14)
    }

    /// High-contrast ring accent for pinned canvas Color Readout markers (`#1..#4`).
    #[inline]
    pub fn pin_marker_ring() -> Color {
        srgb(0.96, 0.56, 0.16)
    }

    /// Phase 4 Rubylith Red Mask Overlay HUD & active accent.
    #[inline]
    pub fn mask_overlay_red() -> Color {
        srgb(0.94, 0.30, 0.30)
    }

    /// Phase 4 Grayscale Mask Overlay HUD accent.
    #[inline]
    pub fn mask_overlay_gray() -> Color {
        srgb(0.84, 0.86, 0.90)
    }

    /// Capture One warm-orange primary vector guide stroke for Linear & Radial gradient masks.
    #[inline]
    pub fn mask_guide_primary() -> Color {
        srgb(0.96, 0.56, 0.16)
    }

    /// Softer feather boundary stroke for Linear & Radial gradient canvas guides.
    #[inline]
    pub fn mask_guide_feather() -> Color {
        srgb(0.96, 0.56, 0.16).alpha(0.58)
    }

    /// Spot Heal target ring accent (warm orange).
    #[inline]
    pub fn heal_target_ring() -> Color {
        srgb(0.96, 0.56, 0.16)
    }

    /// Clone Stamp target ring accent (cool cyan).
    #[inline]
    pub fn clone_target_ring() -> Color {
        srgb(0.32, 0.78, 0.96)
    }

    /// Spot Heal / Clone source patch circle ring.
    #[inline]
    pub fn heal_source_ring() -> Color {
        srgb(0.94, 0.95, 0.97).alpha(0.90)
    }

    /// Dashed connector stroke linking a Heal/Clone target to its source patch.
    #[inline]
    pub fn heal_connector_line() -> Color {
        srgb(0.94, 0.95, 0.97).alpha(0.60)
    }

    /// Phase 5 Studio Proof Margin neutral gallery mat (`#161616`).
    #[inline]
    pub fn proof_margin_studio() -> Color {
        srgb(0.086, 0.086, 0.086)
    }

    /// Phase 5 Exhibition Proof Margin gallery mat (`#1E1E1E`).
    #[inline]
    pub fn proof_margin_exhibition() -> Color {
        srgb(0.118, 0.118, 0.118)
    }

    /// Phase 5 Film Negative C-41 amber HUD & badge accent.
    #[inline]
    pub fn film_negative_accent() -> Color {
        srgb(0.95, 0.56, 0.24)
    }

    /// Converts a Capture One `ColorTag` to a linear `rune::Color` swatch.
    #[inline]
    pub fn color_tag_swatch(tag: rune_editor_core::ColorTag, k: &Kit) -> Color {
        if tag == rune_editor_core::ColorTag::None {
            return k.c.border_strong;
        }
        let [r, g, b] = tag.srgb_u8();
        srgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
    }

    /// Combined RGB Tone Curve channel stroke/dot accent.
    #[inline]
    pub fn channel_rgb() -> Color {
        srgb(0.84, 0.86, 0.90)
    }

    #[inline]
    pub fn wheel_crosshair(k: &Kit) -> Color {
        k.c.white.alpha(0.28)
    }

    #[inline]
    pub fn wheel_tether(k: &Kit) -> Color {
        k.c.white.alpha(0.65)
    }

    #[inline]
    pub fn swatch_rim(k: &Kit) -> Color {
        k.c.white.alpha(0.25)
    }

    #[inline]
    pub fn puck_shadow(k: &Kit) -> Color {
        k.c.black.alpha(0.60)
    }

    // Calibrated scope / histogram channel palette
    #[inline]
    pub fn hist_luma_fill() -> Color {
        srgb(0.84, 0.85, 0.88).alpha(0.18)
    }
    #[inline]
    pub fn hist_luma_stroke() -> Color {
        srgb(0.92, 0.92, 0.94).alpha(0.55)
    }
    #[inline]
    pub fn hist_r_fill() -> Color {
        srgb(0.95, 0.34, 0.34).alpha(0.28)
    }
    #[inline]
    pub fn hist_r_stroke() -> Color {
        srgb(0.96, 0.40, 0.40).alpha(0.82)
    }
    #[inline]
    pub fn hist_g_fill() -> Color {
        srgb(0.32, 0.86, 0.48).alpha(0.26)
    }
    #[inline]
    pub fn hist_g_stroke() -> Color {
        srgb(0.38, 0.88, 0.52).alpha(0.78)
    }
    #[inline]
    pub fn hist_b_fill() -> Color {
        srgb(0.34, 0.58, 0.98).alpha(0.30)
    }
    #[inline]
    pub fn hist_b_stroke() -> Color {
        srgb(0.42, 0.64, 0.98).alpha(0.82)
    }

    // Tone curve & 6-channel B&W indicator dots
    #[inline]
    pub fn channel_red() -> Color {
        srgb(0.95, 0.36, 0.36)
    }
    #[inline]
    pub fn channel_yellow() -> Color {
        srgb(0.94, 0.80, 0.30)
    }
    #[inline]
    pub fn channel_green() -> Color {
        srgb(0.35, 0.86, 0.48)
    }
    #[inline]
    pub fn channel_cyan() -> Color {
        srgb(0.32, 0.82, 0.88)
    }
    #[inline]
    pub fn channel_blue() -> Color {
        srgb(0.38, 0.62, 0.98)
    }
    #[inline]
    pub fn channel_magenta() -> Color {
        srgb(0.86, 0.40, 0.82)
    }

    /// Calibrated 8-band HSL display swatches for OpenCode dark chrome.
    #[inline]
    pub fn hsl_band_swatch(index: usize) -> Color {
        match index {
            0 => srgb(0.92, 0.28, 0.28), // Red
            1 => srgb(0.94, 0.56, 0.22), // Orange
            2 => srgb(0.92, 0.82, 0.26), // Yellow
            3 => srgb(0.32, 0.80, 0.42), // Green
            4 => srgb(0.24, 0.78, 0.84), // Aqua / Cyan
            5 => srgb(0.32, 0.56, 0.96), // Blue
            6 => srgb(0.62, 0.40, 0.94), // Purple
            _ => srgb(0.88, 0.34, 0.72), // Magenta
        }
    }

    /// Neutral puck fill when a color wheel sits at center.
    #[inline]
    pub fn wheel_neutral_puck() -> Color {
        srgb(0.85, 0.85, 0.88)
    }

    /// Live MCP server running status dot & subtle badge tint.
    #[inline]
    pub fn mcp_running() -> Color {
        srgb(0.28, 0.84, 0.48)
    }

    #[inline]
    pub fn mcp_running_bg() -> Color {
        srgb(0.28, 0.84, 0.48).alpha(0.14)
    }

    /// Stopped / offline muted status indicator.
    #[inline]
    pub fn mcp_stopped(k: &Kit) -> Color {
        k.c.text_muted
    }

    /// Code block background surface inside Settings connect snippets.
    #[inline]
    pub fn code_surface(k: &Kit) -> Color {
        k.c.bg_deep
    }
}
