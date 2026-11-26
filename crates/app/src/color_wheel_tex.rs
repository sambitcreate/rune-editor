//! High-DPI circular color wheel texture and geometry helpers for the
//! Capture One-style 3-Way Color Balance inspector.

use std::f32::consts::TAU;
use std::sync::OnceLock;

use rune::{Color, Pixels};

use crate::tokens;

/// Returns a cached 288x288 anti-aliased Capture One-style color balance wheel texture
/// (crisp at 2x Retina for both 3-Way 88pt wheels and the 148pt Master wheel).
///
/// The wheel maps angle -> hue (0 deg = Red at +X, 120 deg = Green, 240 deg = Blue)
/// and radius -> saturation with a neutral studio gray center, darkened smoothly so
/// white/amber puck indicators remain high-contrast across the entire disc.
pub fn wheel_pixels() -> &'static Pixels {
    static PIXELS: OnceLock<Pixels> = OnceLock::new();
    PIXELS.get_or_init(|| {
        let size: u32 = 288;
        let rgba = build_color_wheel_rgba(size);
        let px = Pixels::new();
        px.set(size, size, rgba);
        px
    })
}

/// Generates an RGBA8 circular color balance wheel of `size x size` pixels.
pub fn build_color_wheel_rgba(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size as usize) * (size as usize) * 4];
    let half = (size as f32) * 0.5;
    let outer_r = half - 1.5;

    for y in 0..size {
        for x in 0..size {
            let dx = (x as f32 + 0.5 - half) / outer_r;
            let dy = (y as f32 + 0.5 - half) / outer_r;
            let r = (dx * dx + dy * dy).sqrt();

            let edge_px = (1.0 - r) * outer_r;
            let alpha = (edge_px + 0.75).clamp(0.0, 1.0);
            if alpha <= 0.0 {
                continue;
            }

            let angle = (-dy).atan2(dx).rem_euclid(TAU);
            let hue = angle / TAU;

            let sat_curve = r.clamp(0.0, 1.0).powf(1.15);
            let is_rim = ((r - 0.88) / 0.12).clamp(0.0, 1.0);
            let (hr, hg, hb) = hsv_to_rgb(hue, 0.82, 0.88);

            let base = 0.145;
            let mix_amt = sat_curve * 0.56 + is_rim * 0.28;
            let cr = (base + (hr - base) * mix_amt).clamp(0.0, 1.0);
            let cg = (base + (hg - base) * mix_amt).clamp(0.0, 1.0);
            let cb = (base + (hb - base) * mix_amt).clamp(0.0, 1.0);

            let idx = ((y * size + x) * 4) as usize;
            out[idx] = (cr * 255.0).round() as u8;
            out[idx + 1] = (cg * 255.0).round() as u8;
            out[idx + 2] = (cb * 255.0).round() as u8;
            out[idx + 3] = (alpha * 255.0).round() as u8;
        }
    }
    out
}

/// Converts HSV (`h`, `s`, `v` in `0.0..=1.0`) to sRGB (`0.0..=1.0`).
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h6 = (h.rem_euclid(1.0)) * 6.0;
    let sector = h6.floor() as i32;
    let f = h6 - sector as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    match sector.rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

/// Maps pointer `(px, py)` in a `diameter x diameter` wheel widget to normalized
/// `(hue_x, hue_y)` clamped to the unit disk (`x^2 + y^2 <= 1.0`).
pub fn pointer_to_wheel_xy(px: f32, py: f32, diameter: f32) -> (f32, f32) {
    let r = (diameter * 0.5).max(1.0);
    let usable = (r - 4.0).max(1.0);
    let mut x = (px - r) / usable;
    let mut y = -(py - r) / usable;
    let mag = (x * x + y * y).sqrt();
    if mag > 1.0 {
        x /= mag;
        y /= mag;
    }
    (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0))
}

/// Maps normalized `(hue_x, hue_y)` in the unit disk back to widget-local `(px, py)`.
pub fn wheel_xy_to_point(hue_x: f32, hue_y: f32, diameter: f32) -> [f32; 2] {
    let r = diameter * 0.5;
    let usable = (r - 4.0).max(1.0);
    [r + hue_x * usable, r - hue_y * usable]
}

/// Computes a preview tint color for a color wheel puck given `(hue_x, hue_y)`.
pub fn wheel_puck_color(hue_x: f32, hue_y: f32) -> Color {
    let mag = (hue_x * hue_x + hue_y * hue_y).sqrt().clamp(0.0, 1.0);
    if mag < 0.02 {
        return tokens::color::wheel_neutral_puck();
    }
    let hue = hue_y.atan2(hue_x).rem_euclid(TAU) / TAU;
    let (r, g, b) = hsv_to_rgb(hue, 0.35 + 0.65 * mag, 0.96);
    tokens::srgb(r, g, b)
}
