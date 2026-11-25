//! Image decoding and scene-referred linear `Rgba16Float` buffer preparation.
//!
//! Converts standard photo formats (JPEG, PNG, TIFF, WebP) into 16-bit half-float
//! linear textures ready for direct upload into the GPU develop pipeline, plus a
//! downsampled linear proxy for sub-millisecond live histogram evaluation.

use std::path::{Path, PathBuf};

use half::f16;
use rune_editor_core::srgb_to_linear;
use thiserror::Error;

/// Decoded linear `Rgba16Float` image buffer plus metadata and fast histogram proxy.
#[derive(Clone, Debug)]
pub struct LinearImage {
    pub width: u32,
    pub height: u32,
    /// Interleaved `RGBA16Float` pixels (`width * height * 4` `f16` values).
    pub rgba16f: Vec<f16>,
    /// Downsampled linear RGB samples for instant live histogram calculation.
    pub histogram_proxy: Vec<[f32; 3]>,
    pub display_name: String,
    pub source_path: Option<PathBuf>,
    pub format_label: String,
}

impl LinearImage {
    pub fn byte_slice(&self) -> &[u8] {
        bytemuck::cast_slice(&self.rgba16f)
    }
}

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("Failed to open image '{path}': {source}")]
    Image {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },
    #[error("Empty image dimensions ({0}x{1})")]
    EmptyDimensions(u32, u32),
}

/// Decodes a photo file from disk into scene-referred linear `Rgba16Float`.
pub fn load_file(path: &Path) -> Result<LinearImage, DecodeError> {
    let dyn_img = image::open(path).map_err(|source| DecodeError::Image {
        path: path.to_path_buf(),
        source,
    })?;
    let rgba32f = dyn_img.to_rgba32f();
    let (width, height) = rgba32f.dimensions();
    if width == 0 || height == 0 {
        return Err(DecodeError::EmptyDimensions(width, height));
    }

    let pixel_count = (width as usize) * (height as usize);
    let mut rgba16f = Vec::with_capacity(pixel_count * 4);
    for px in rgba32f.pixels() {
        let r = srgb_to_linear(px[0]);
        let g = srgb_to_linear(px[1]);
        let b = srgb_to_linear(px[2]);
        let a = px[3].clamp(0.0, 1.0);
        rgba16f.push(f16::from_f32(r));
        rgba16f.push(f16::from_f32(g));
        rgba16f.push(f16::from_f32(b));
        rgba16f.push(f16::from_f32(a));
    }

    let histogram_proxy = build_histogram_proxy(&rgba16f, width, height);
    let display_name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_owned());
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_uppercase())
        .unwrap_or_else(|| "IMAGE".to_owned());

    Ok(LinearImage {
        width,
        height,
        rgba16f,
        histogram_proxy,
        display_name,
        source_path: Some(path.to_path_buf()),
        format_label: format!("{ext} • 16-bit Linear FP16"),
    })
}

/// Generates a rich procedural HDR scene-referred studio test photograph (alpine golden hour
/// with >1.0 HDR sun/sky highlights, cool mountain shadows, warm foreground cabin glow, and
/// soft clouds) so the darkroom opens immediately with real dynamic range to test every slider.
pub fn generate_demo_hdr_scene(width: u32, height: u32) -> LinearImage {
    let mut rgba16f = Vec::with_capacity((width as usize) * (height as usize) * 4);

    let wf = width.max(1) as f32;
    let hf = height.max(1) as f32;

    for y in 0..height {
        let v = y as f32 / hf; // 0 = top, 1 = bottom
        for x in 0..width {
            let u = x as f32 / wf; // 0 = left, 1 = right

            // Sky gradient in scene-referred linear light (HDR zenith -> glowing horizon)
            let horizon = (1.0 - (v * 1.45).clamp(0.0, 1.0)).powf(1.8);
            let mut r = 0.04 + 0.95 * (1.0 - horizon).powf(2.2) * (1.0 - v).max(0.0) * 1.3;
            let mut g = 0.08 + 0.36 * (1.0 - horizon).powf(2.0) * (1.0 - v).max(0.0);
            let mut b = 0.24 + 0.20 * horizon;

            // Warm HDR golden-hour sun glow (values > 1.0 exercise Highlights recovery!)
            let sx = u - 0.68;
            let sy = (v - 0.34) * (hf / wf);
            let sun_dist = (sx * sx + sy * sy).sqrt();
            let sun_core = (-sun_dist * 24.0).exp() * 2.4;
            let sun_halo = (-sun_dist * 4.5).exp() * 0.72;
            r += sun_core * 1.35 + sun_halo * 1.10;
            g += sun_core * 0.95 + sun_halo * 0.55;
            b += sun_core * 0.42 + sun_halo * 0.18;

            // Soft backlit cloud bands
            let cloud_wave = ((u * 9.0 + v * 4.0).sin() * 0.5 + (u * 21.0 - v * 7.0).cos() * 0.25)
                * (-(v - 0.24).powi(2) * 90.0).exp();
            if cloud_wave > 0.05 {
                let cw = (cloud_wave - 0.05) * 0.9;
                r += cw * 0.65;
                g += cw * 0.38;
                b += cw * 0.24;
            }

            // Distant snowy mountain ridge
            let ridge1 = 0.48
                - 0.13 * (u * 3.1 + 0.4).sin().abs()
                - 0.05 * (u * 8.7 + 1.2).cos()
                - 0.02 * (u * 23.0).sin();
            if v > ridge1 {
                let depth = ((v - ridge1) * 14.0).clamp(0.0, 1.0);
                // Sunlit snow faces on right-facing slopes, cool blue shadow faces on left
                let slope = (u * 3.1 + 0.4).cos() + 0.4 * (u * 8.7 + 1.2).sin();
                let snow = (1.0 - depth * 0.85).clamp(0.0, 1.0);
                let sunlit = (slope * 0.5 + 0.5).clamp(0.0, 1.0);
                r = 0.03 + snow * (0.14 + 0.82 * sunlit);
                g = 0.05 + snow * (0.18 + 0.58 * sunlit);
                b = 0.10 + snow * (0.28 + 0.32 * sunlit);
            }

            // Midground alpine pine ridge (rich shadow detail to test Shadows lift!)
            let ridge2 = 0.62
                - 0.06 * (u * 4.5 + 2.0).sin()
                - 0.025 * (u * 13.0).cos()
                - 0.008 * (u * 59.0).sin().abs();
            if v > ridge2 {
                let tree_tex = 0.82 + 0.18 * (u * 140.0 + (v * 35.0).sin()).sin();
                let mist = (-((v - ridge2) * 18.0)).exp() * 0.10;
                r = (0.015 + mist * 0.85) * tree_tex;
                g = (0.032 + mist * 0.72) * tree_tex;
                b = (0.028 + mist * 0.80) * tree_tex;
            }

            // Reflective alpine lake in lower third
            let shore = 0.74 + 0.012 * (u * 7.0).sin();
            if v > shore {
                let lake_t = ((v - shore) / (1.0 - shore)).clamp(0.0, 1.0);
                let ripple = (u * 55.0 + v * 210.0).sin() * 0.015;
                let rx = u + ripple;
                let ref_sun = (-((rx - 0.68).powi(2) * 32.0 + (lake_t - 0.25).powi(2) * 6.0)).exp();
                r = 0.025 + ref_sun * 0.95 + (1.0 - lake_t) * 0.08;
                g = 0.045 + ref_sun * 0.50 + (1.0 - lake_t) * 0.06;
                b = 0.085 + ref_sun * 0.18 + (1.0 - lake_t) * 0.10;
            }

            // Warm golden cabin window light on the left shore (vivid point contrast)
            let cx = u - 0.26;
            let cy = (v - 0.715) * (hf / wf);
            let cabin_d = (cx * cx + cy * cy).sqrt();
            let lantern = (-cabin_d * 52.0).exp() * 1.45;
            r += lantern * 1.15;
            g += lantern * 0.62;
            b += lantern * 0.16;

            rgba16f.push(f16::from_f32(r.max(0.0)));
            rgba16f.push(f16::from_f32(g.max(0.0)));
            rgba16f.push(f16::from_f32(b.max(0.0)));
            rgba16f.push(f16::from_f32(1.0));
        }
    }

    let histogram_proxy = build_histogram_proxy(&rgba16f, width, height);
    LinearImage {
        width,
        height,
        rgba16f,
        histogram_proxy,
        display_name: "Alpine_Twilight_HDR.dng (Demo)".to_owned(),
        source_path: None,
        format_label: "HDR Demo • 16-bit Linear FP16".to_owned(),
    }
}

fn build_histogram_proxy(rgba16f: &[f16], width: u32, height: u32) -> Vec<[f32; 3]> {
    let target_w = 160u32.min(width.max(1));
    let target_h = 106u32.min(height.max(1));
    let mut proxy = Vec::with_capacity((target_w * target_h) as usize);
    for py in 0..target_h {
        let sy = ((py as u64 * height as u64) / target_h as u64).min(height as u64 - 1) as usize;
        for px in 0..target_w {
            let sx = ((px as u64 * width as u64) / target_w as u64).min(width as u64 - 1) as usize;
            let idx = (sy * width as usize + sx) * 4;
            proxy.push([
                rgba16f[idx].to_f32(),
                rgba16f[idx + 1].to_f32(),
                rgba16f[idx + 2].to_f32(),
            ]);
        }
    }
    proxy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_scene_has_hdr_dynamic_range() {
        let img = generate_demo_hdr_scene(320, 200);
        assert_eq!(img.rgba16f.len(), 320 * 200 * 4);
        let max_val = img
            .rgba16f
            .iter()
            .step_by(4)
            .map(|h| h.to_f32())
            .fold(0.0_f32, f32::max);
        assert!(max_val > 1.2, "demo scene should contain HDR >1.0 highlights, got {max_val}");
    }
}
