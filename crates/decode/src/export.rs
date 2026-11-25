//! Full-resolution and web-sized multi-core photo export (JPEG, PNG8, PNG16, TIFF16, WebP).
//!
//! Evaluates scene-referred linear `Rgba16Float` pixels through optional geometry/crop,
//! high-quality linear-light Catmull-Rom downscaling, `rune_editor_core::CompiledRecipe`
//! plus optical vignette and subtle film grain matching `shaders/develop.wgsl`, filmic
//! tonemapping, optional screen output sharpening, and sRGB 8-bit/16-bit encoding.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use half::f16;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, ImageBuffer, ImageFormat, Rgb};
use rayon::prelude::*;
use rune_editor_core::{CompiledRecipe, DevelopRecipe, filmic_tonemap, linear_to_srgb};

use crate::raw::fit_dimensions;
use crate::{DecodeError, LinearImage};

/// Output container and bit-depth for photo export.
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
    /// 8-bit sRGB lossless WebP.
    WebP,
}

/// Configuration options for [`export_image_with_options`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExportOptions {
    pub format: ExportFormat,
    /// Optional longest-edge cap in pixels (e.g. `Some(2048)` for web export).
    pub max_edge: Option<u32>,
    /// When `true`, applies `recipe.geometry` (rotate, flip, straighten, crop).
    pub respect_crop: bool,
    /// Post-resize screen output sharpening strength in `0.0..=1.0`.
    pub output_sharpen: f32,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            format: ExportFormat::Jpeg { quality: 92 },
            max_edge: None,
            respect_crop: true,
            output_sharpen: 0.0,
        }
    }
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
/// high-quality downscaling, output sharpening, and format encoding) and writes to `dest`.
pub fn export_image_with_options(
    image: &LinearImage,
    recipe: &DevelopRecipe,
    dest: &Path,
    options: &ExportOptions,
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

    // Step 1: Optional linear-space geometry/crop and Catmull-Rom downscaling.
    let (out_w, out_h, resized_linear) = prepare_linear_source(image, recipe, options);
    let src = match resized_linear.as_deref() {
        Some(buf) => LinearSource::RgbF32(buf),
        None => LinearSource::Rgba16F(&image.rgba16f),
    };

    let sharpen = if options.output_sharpen.is_finite() {
        options.output_sharpen.clamp(0.0, 1.0)
    } else {
        0.0
    };

    let file = File::create(dest).map_err(|source| DecodeError::Io {
        path: dest.to_path_buf(),
        source,
    })?;
    let mut writer = BufWriter::new(file);

    match options.format {
        ExportFormat::Jpeg { quality } => {
            let rgb8 = render_export_quantized(src, out_w, out_h, recipe, sharpen, to_u8);
            let mut encoder = JpegEncoder::new_with_quality(&mut writer, quality.clamp(1, 100));
            encoder
                .encode(&rgb8, out_w, out_h, ExtendedColorType::Rgb8)
                .map_err(|source| DecodeError::Image {
                    path: dest.to_path_buf(),
                    source,
                })?;
        }
        ExportFormat::Png8 => {
            let rgb8 = render_export_quantized(src, out_w, out_h, recipe, sharpen, to_u8);
            let buf: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_raw(out_w, out_h, rgb8)
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
            let rgb16 = render_export_quantized(src, out_w, out_h, recipe, sharpen, to_u16);
            let buf: ImageBuffer<Rgb<u16>, Vec<u16>> = ImageBuffer::from_raw(out_w, out_h, rgb16)
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
            let rgb16 = render_export_quantized(src, out_w, out_h, recipe, sharpen, to_u16);
            let buf: ImageBuffer<Rgb<u16>, Vec<u16>> = ImageBuffer::from_raw(out_w, out_h, rgb16)
                .ok_or_else(|| {
                DecodeError::InvalidBuffer("Failed to construct Rgb16 ImageBuffer".to_owned())
            })?;
            buf.write_to(&mut writer, ImageFormat::Tiff)
                .map_err(|source| DecodeError::Image {
                    path: dest.to_path_buf(),
                    source,
                })?;
        }
        ExportFormat::WebP => {
            let rgb8 = render_export_quantized(src, out_w, out_h, recipe, sharpen, to_u8);
            WebPEncoder::new_lossless(&mut writer)
                .encode(&rgb8, out_w, out_h, ExtendedColorType::Rgb8)
                .map_err(|source| DecodeError::Image {
                    path: dest.to_path_buf(),
                    source,
                })?;
        }
    }

    Ok(())
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
}

fn prepare_linear_source(
    image: &LinearImage,
    recipe: &DevelopRecipe,
    options: &ExportOptions,
) -> (u32, u32, Option<Vec<[f32; 3]>>) {
    let mut cur_w = image.width;
    let mut cur_h = image.height;
    let mut owned: Option<Vec<[f32; 3]>> = None;

    if options.respect_crop && !recipe.geometry.is_identity() {
        let (geo_w, geo_h, transformed) = apply_geometry_linear(image, recipe);
        cur_w = geo_w;
        cur_h = geo_h;
        owned = Some(transformed);
    }

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

    (cur_w, cur_h, owned)
}

/// Applies persistent geometry (`rotate_90`, `flip_h`, `flip_v`, `straighten_deg`, and `crop`)
/// in scene-referred linear light with exact UV parity to `shaders/develop.wgsl`.
fn apply_geometry_linear(image: &LinearImage, recipe: &DevelopRecipe) -> (u32, u32, Vec<[f32; 3]>) {
    let geo = &recipe.geometry;
    let [out_w, out_h] = geo.cropped_output_size(image.width, image.height);
    let ow = out_w as usize;
    let oh = out_h as usize;
    let owf = out_w as f32;
    let ohf = out_h as f32;

    let mut out = vec![[0.0f32; 3]; ow * oh];
    out.par_chunks_exact_mut(ow)
        .enumerate()
        .for_each(|(y, dst_row)| {
            let out_v = (y as f32 + 0.5) / ohf;
            for (x, dst_px) in dst_row.iter_mut().enumerate() {
                let out_u = (x as f32 + 0.5) / owf;
                if let Some([su, sv]) =
                    geo.cropped_to_source_uv(out_u, out_v, image.width, image.height)
                {
                    *dst_px =
                        sample_bilinear_rgba16f(&image.rgba16f, image.width, image.height, su, sv);
                }
            }
        });

    (out_w, out_h, out)
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

fn render_export_quantized<T: Copy + Default + Send + Sync>(
    src: LinearSource<'_>,
    width: u32,
    height: u32,
    recipe: &DevelopRecipe,
    sharpen: f32,
    quantize: impl Fn(f32) -> T + Sync,
) -> Vec<T> {
    let w = width as usize;
    let h = height as usize;

    if sharpen > 0.001 && w >= 2 && h >= 2 {
        let mut srgb = vec![[0.0f32; 3]; w * h];
        render_rows_par(&mut srgb, w, src, width, height, recipe, |row, x, px| {
            row[x] = px;
        });
        return sharpen_and_quantize(&srgb, w, h, sharpen, quantize);
    }

    let mut out = vec![T::default(); w * h * 3];
    render_rows_par(&mut out, w * 3, src, width, height, recipe, |row, x, px| {
        let didx = x * 3;
        row[didx] = quantize(px[0]);
        row[didx + 1] = quantize(px[1]);
        row[didx + 2] = quantize(px[2]);
    });
    out
}

fn render_rows_par<T: Send>(
    out: &mut [T],
    row_stride: usize,
    src: LinearSource<'_>,
    width: u32,
    height: u32,
    recipe: &DevelopRecipe,
    write_px: impl Fn(&mut [T], usize, [f32; 3]) + Sync,
) {
    let compiled = recipe.compile();
    let w = width as usize;
    let wf = width.max(1) as f32;
    let hf = height.max(1) as f32;

    out.par_chunks_exact_mut(row_stride)
        .enumerate()
        .for_each(|(y, dst_row)| {
            let row_base = y * w;
            let v = (y as f32 + 0.5) / hf;
            for x in 0..w {
                let u = (x as f32 + 0.5) / wf;
                let srgb = evaluate_export_pixel_srgb(
                    src.get_rgb(row_base + x),
                    u,
                    v,
                    x as f32,
                    y as f32,
                    recipe,
                    &compiled,
                );
                write_px(dst_row, x, srgb);
            }
        });
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
