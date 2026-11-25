//! Full-resolution multi-core photo export (8-bit JPEG/PNG and 16-bit PNG/TIFF).
//!
//! Evaluates every scene-referred linear `Rgba16Float` pixel through
//! `rune_editor_core::evaluate_pixel_linear` plus optical vignette and subtle
//! photographic film grain matching `shaders/develop.wgsl`, then encodes into
//! sRGB 8-bit or 16-bit output files.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::{ImageBuffer, ImageFormat, Rgb};
use rayon::prelude::*;
use rune_editor_core::{CompiledRecipe, DevelopRecipe, filmic_tonemap, linear_to_srgb};

use crate::{DecodeError, LinearImage};

/// Output container and bit-depth for full-resolution photo export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    /// 8-bit sRGB JPEG (`quality` clamped to `1..=100`).
    Jpeg { quality: u8 },
    /// 16-bit sRGB lossless PNG.
    Png16,
    /// 8-bit sRGB lossless PNG.
    Png8,
    /// 16-bit sRGB TIFF.
    Tiff16,
}

/// Evaluates `image` through `recipe` at full resolution and writes the encoded
/// result to `dest` in the requested [`ExportFormat`].
pub fn export_image(
    image: &LinearImage,
    recipe: &DevelopRecipe,
    dest: &Path,
    format: ExportFormat,
) -> Result<(), DecodeError> {
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

    let file = File::create(dest).map_err(|source| DecodeError::Io {
        path: dest.to_path_buf(),
        source,
    })?;
    let mut writer = BufWriter::new(file);

    match format {
        ExportFormat::Jpeg { quality } => {
            let rgb8 = render_srgb8_buffer(image, recipe);
            let mut encoder = JpegEncoder::new_with_quality(&mut writer, quality.clamp(1, 100));
            encoder
                .encode(&rgb8, width, height, image::ExtendedColorType::Rgb8)
                .map_err(|source| DecodeError::Image {
                    path: dest.to_path_buf(),
                    source,
                })?;
        }
        ExportFormat::Png8 => {
            let rgb8 = render_srgb8_buffer(image, recipe);
            let buf: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_raw(width, height, rgb8)
                .ok_or_else(|| {
                    DecodeError::InvalidBuffer("Failed to construct Rgb8 ImageBuffer".to_owned())
                })?;
            buf.write_to(&mut writer, ImageFormat::Png)
                .map_err(|source| DecodeError::Image {
                    path: dest.to_path_buf(),
                    source,
                })?;
        }
        ExportFormat::Png16 => {
            let rgb16 = render_srgb16_buffer(image, recipe);
            let buf: ImageBuffer<Rgb<u16>, Vec<u16>> = ImageBuffer::from_raw(width, height, rgb16)
                .ok_or_else(|| {
                    DecodeError::InvalidBuffer("Failed to construct Rgb16 ImageBuffer".to_owned())
                })?;
            buf.write_to(&mut writer, ImageFormat::Png)
                .map_err(|source| DecodeError::Image {
                    path: dest.to_path_buf(),
                    source,
                })?;
        }
        ExportFormat::Tiff16 => {
            let rgb16 = render_srgb16_buffer(image, recipe);
            let buf: ImageBuffer<Rgb<u16>, Vec<u16>> = ImageBuffer::from_raw(width, height, rgb16)
                .ok_or_else(|| {
                    DecodeError::InvalidBuffer("Failed to construct Rgb16 ImageBuffer".to_owned())
                })?;
            buf.write_to(&mut writer, ImageFormat::Tiff)
                .map_err(|source| DecodeError::Image {
                    path: dest.to_path_buf(),
                    source,
                })?;
        }
    }

    Ok(())
}

fn render_srgb8_buffer(image: &LinearImage, recipe: &DevelopRecipe) -> Vec<u8> {
    let compiled = recipe.compile();
    let w = image.width as usize;
    let h = image.height as usize;
    let wf = image.width.max(1) as f32;
    let hf = image.height.max(1) as f32;
    let mut out = vec![0u8; w * h * 3];

    out.par_chunks_exact_mut(w * 3)
        .enumerate()
        .for_each(|(y, dst_row)| {
            let src_row_base = y * w * 4;
            let v = (y as f32 + 0.5) / hf;
            for x in 0..w {
                let sidx = src_row_base + x * 4;
                let src = [
                    image.rgba16f[sidx].to_f32(),
                    image.rgba16f[sidx + 1].to_f32(),
                    image.rgba16f[sidx + 2].to_f32(),
                ];
                let u = (x as f32 + 0.5) / wf;
                let srgb =
                    evaluate_export_pixel_srgb(src, u, v, x as f32, y as f32, recipe, &compiled);
                let didx = x * 3;
                dst_row[didx] = (srgb[0] * 255.0).round().clamp(0.0, 255.0) as u8;
                dst_row[didx + 1] = (srgb[1] * 255.0).round().clamp(0.0, 255.0) as u8;
                dst_row[didx + 2] = (srgb[2] * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        });

    out
}

fn render_srgb16_buffer(image: &LinearImage, recipe: &DevelopRecipe) -> Vec<u16> {
    let compiled = recipe.compile();
    let w = image.width as usize;
    let h = image.height as usize;
    let wf = image.width.max(1) as f32;
    let hf = image.height.max(1) as f32;
    let mut out = vec![0u16; w * h * 3];

    out.par_chunks_exact_mut(w * 3)
        .enumerate()
        .for_each(|(y, dst_row)| {
            let src_row_base = y * w * 4;
            let v = (y as f32 + 0.5) / hf;
            for x in 0..w {
                let sidx = src_row_base + x * 4;
                let src = [
                    image.rgba16f[sidx].to_f32(),
                    image.rgba16f[sidx + 1].to_f32(),
                    image.rgba16f[sidx + 2].to_f32(),
                ];
                let u = (x as f32 + 0.5) / wf;
                let srgb =
                    evaluate_export_pixel_srgb(src, u, v, x as f32, y as f32, recipe, &compiled);
                let didx = x * 3;
                dst_row[didx] = (srgb[0] * 65535.0).round().clamp(0.0, 65535.0) as u16;
                dst_row[didx + 1] = (srgb[1] * 65535.0).round().clamp(0.0, 65535.0) as u16;
                dst_row[didx + 2] = (srgb[2] * 65535.0).round().clamp(0.0, 65535.0) as u16;
            }
        });

    out
}

/// Evaluates a single linear RGB pixel using a precompiled [`CompiledRecipe`] in
/// scene-referred linear light (stages 1–7), applies optical radial vignette and
/// subtle midtone-weighted film grain pre-tonemap (matching `shaders/develop.wgsl`),
/// then applies [`filmic_tonemap`] and encodes into `[0.0, 1.0]` sRGB.
#[inline]
fn evaluate_export_pixel_srgb(
    src_rgb: [f32; 3],
    u: f32,
    v: f32,
    px_x: f32,
    px_y: f32,
    recipe: &DevelopRecipe,
    compiled: &CompiledRecipe,
) -> [f32; 3] {
    let mut rgb = compiled.evaluate_pixel_scene_linear(src_rgb);

    // Optical radial vignette in scene-referred linear light (matches shaders/develop.wgsl).
    if recipe.vignette.abs() > 1e-5 {
        let cx = (u - 0.5) * 2.0;
        let cy = (v - 0.5) * 2.0;
        let r2 = cx * cx + cy * cy;
        let vig_gain = 2.0_f32.powf(recipe.vignette * 1.15 * smoothstep(0.18, 1.65, r2));
        rgb[0] *= vig_gain;
        rgb[1] *= vig_gain;
        rgb[2] *= vig_gain;
    }

    // Subtle photographic film grain pre-tonemap (stronger in midtones; matches shaders/develop.wgsl).
    if recipe.grain > 0.001 {
        let luma = (0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]).max(1e-5);
        let t = (luma / (luma + 0.18)).clamp(0.0, 1.0);
        let n = (hash12(px_x, px_y) - 0.5) * 2.0;
        let mid_mask = 1.0 - (2.0 * t - 1.0).abs();
        let delta = n * recipe.grain * 0.065 * (0.35 + 0.65 * mid_mask);
        rgb[0] = (rgb[0] + delta).max(0.0);
        rgb[1] = (rgb[1] + delta).max(0.0);
        rgb[2] = (rgb[2] + delta).max(0.0);
    }

    let mapped = filmic_tonemap(rgb);
    [
        linear_to_srgb(mapped[0]),
        linear_to_srgb(mapped[1]),
        linear_to_srgb(mapped[2]),
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
