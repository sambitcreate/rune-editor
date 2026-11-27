//! Camera RAW, ProRAW, HEIC, and raster photo decoding into scene-referred linear `Rgba16Float`.
//!
//! Decoding uses a two-tier architecture:
//! - **Tier 1 (macOS):** Apple's native `CIRAWFilter` (CoreImage) + `ImageIO` for hardware-accelerated
//!   RAW, ProRAW, DNG, and HEIC/HEIF decoding directly into `kCIFormatRGBAh` extended linear sRGB,
//!   with native `scaleFactor` proxy decoding for sub-second interactive editing.
//! - **Tier 2 (Cross-platform fallback):** Pure-Rust `rawler` decoding with camera white balance,
//!   black/white level normalization, CFA demosaicing (PPG / X-Trans / bilinear fallback), and
//!   linear camera-to-sRGB calibration.

use std::path::Path;

use half::f16;
use rayon::prelude::*;
use rune_editor_core::{CameraCalibration, srgb_to_linear};

use crate::{DecodeError, LinearImage, build_histogram_proxy};

/// Controls how Apple `CIRAWFilter` renders camera RAW files on macOS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RawDecodeMode {
    /// True scene-referred linear RAW decode (`boostAmount = 0.0`, `localToneMapAmount = 0.0`,
    /// `contrastAmount = 0.0`) so Rune's `FilmCurve` and 3D camera profile LUT are never double-applied.
    #[default]
    Linear,
    /// Apple Native RAW rendering preserving `CIRAWFilter`'s default photographic tone boost (`apple_raw`).
    AppleNativeBoost,
}

/// Camera RAW & high-efficiency container extensions handled by the RAW / native OS decoder tier.
pub const RAW_EXTENSIONS: &[&str] = &[
    "dng", "cr2", "cr3", "nef", "arw", "raf", "orf", "rw2", "pef", "srw", "heic", "heif",
];

/// Standard raster photo extensions supported natively across all platforms.
pub const RASTER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "webp"];

/// All supported photo extensions (lowercase, without leading dot).
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "dng", "cr2", "cr3", "nef", "arw", "raf", "orf", "rw2", "pef", "srw", "heic", "heif", "jpg",
    "jpeg", "png", "tif", "tiff", "webp",
];

/// Returns `true` if `path` has a supported photo file extension (case-insensitive).
pub fn is_supported_photo(path: &Path) -> bool {
    let Some(ext) = extension_lowercase(path) else {
        return false;
    };
    SUPPORTED_EXTENSIONS.iter().any(|&e| e == ext)
}

/// Returns `true` if `path` has a Camera RAW or HEIC/HEIF extension (case-insensitive).
pub fn is_raw_extension(path: &Path) -> bool {
    let Some(ext) = extension_lowercase(path) else {
        return false;
    };
    RAW_EXTENSIONS.iter().any(|&e| e == ext)
}

pub(crate) fn is_heif_extension(path: &Path) -> bool {
    matches!(extension_lowercase(path).as_deref(), Some("heic" | "heif"))
}

pub(crate) fn extension_lowercase(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

/// Fast header-only dimension probe (`[width, height]` in upright oriented pixels)
/// with zero pixel decoding (`<1ms` on macOS via `ImageIO`).
///
/// Respects EXIF orientation transpose (`5..=8`), falling back cleanly to
/// `image::image_dimensions` and `rawler` dummy header probing.
pub fn probe_image_dimensions(path: &Path) -> Result<[u32; 2], DecodeError> {
    if !path.exists() {
        return Err(DecodeError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "Image file not found"),
        });
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(dims) = macos::probe_imageio_dimensions(path) {
            return Ok(dims);
        }
    }

    if !is_raw_extension(path)
        && let Ok((w, h)) = image::image_dimensions(path)
        && w > 0
        && h > 0
    {
        return Ok([w, h]);
    }

    if is_raw_extension(path)
        && !is_heif_extension(path)
        && let Ok(dims) = probe_rawler_dimensions(path)
    {
        return Ok(dims);
    }

    // Last-resort raster probe (handles misnamed files).
    let (w, h) = image::image_dimensions(path).map_err(|source| DecodeError::Image {
        path: path.to_path_buf(),
        source,
    })?;
    if w == 0 || h == 0 {
        return Err(DecodeError::EmptyDimensions(w, h));
    }
    Ok([w, h])
}

fn probe_rawler_dimensions(path: &Path) -> Result<[u32; 2], DecodeError> {
    use rawler::decoders::RawDecodeParams;
    use rawler::rawsource::RawSource;

    let raw_source = RawSource::new(path).map_err(|source| DecodeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let decoder = rawler::get_decoder(&raw_source).map_err(|e| DecodeError::Raw {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    let raw_img = decoder
        .raw_image(&raw_source, &RawDecodeParams::default(), true)
        .map_err(|e| DecodeError::Raw {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
    let w = raw_img.width as u32;
    let h = raw_img.height as u32;
    if w == 0 || h == 0 {
        return Err(DecodeError::EmptyDimensions(w, h));
    }
    let (transpose, _, _) = raw_img.orientation.to_flips();
    if transpose { Ok([h, w]) } else { Ok([w, h]) }
}

/// Decodes a photo file from disk into scene-referred linear `Rgba16Float`,
/// optionally scaling the longest edge to `max_edge` for fast interactive editing proxies.
///
/// Uses true linear RAW decoding ([`RawDecodeMode::Linear`]) so Rune's `FilmCurve` and
/// 3D camera profile LUT do not double-apply an OS tone curve.
pub fn load_file_proxy(path: &Path, max_edge: Option<u32>) -> Result<LinearImage, DecodeError> {
    load_file_proxy_with_mode(path, max_edge, RawDecodeMode::Linear)
}

/// Decodes a photo file from disk into scene-referred linear `Rgba16Float` with an explicit
/// [`RawDecodeMode`] (`Linear` default, or `AppleNativeBoost` for Apple Native RAW rendering).
pub fn load_file_proxy_with_mode(
    path: &Path,
    max_edge: Option<u32>,
    mode: RawDecodeMode,
) -> Result<LinearImage, DecodeError> {
    let max_edge = max_edge.map(|m| m.max(1));

    if is_raw_extension(path) {
        return decode_raw_or_heif(path, max_edge, mode);
    }

    match decode_raster_image(path, max_edge) {
        Ok(img) => Ok(img),
        Err(raster_err) => {
            // Gracefully handle misnamed HEIC/RAW files on macOS via CoreImage before failing.
            #[cfg(target_os = "macos")]
            {
                if let Ok(img) =
                    macos::decode_coreimage_linear_with_mode(path, max_edge, false, mode)
                {
                    return Ok(img);
                }
            }
            Err(raster_err)
        }
    }
}

fn decode_raw_or_heif(
    path: &Path,
    max_edge: Option<u32>,
    mode: RawDecodeMode,
) -> Result<LinearImage, DecodeError> {
    let is_heif = is_heif_extension(path);

    // Tier 1: Apple native CoreImage (CIRAWFilter for RAW/ProRAW, CIImage for HEIC/HEIF).
    #[cfg(target_os = "macos")]
    {
        match macos::decode_coreimage_linear_with_mode(path, max_edge, !is_heif, mode) {
            Ok(img) => Ok(img),
            Err(macos_err) => {
                if is_heif {
                    return Err(macos_err);
                }
                // Fall through to Tier 2 pure-Rust rawler decoder for unrecognized cameras.
                decode_rawler_linear(path, max_edge).map_err(|rawler_err| DecodeError::Raw {
                    path: path.to_path_buf(),
                    message: format!(
                        "macOS CoreImage failed ({macos_err}); rawler fallback failed ({rawler_err})"
                    ),
                })
            }
        }
    }

    // Tier 2: Pure-Rust rawler fallback on non-macOS platforms.
    #[cfg(not(target_os = "macos"))]
    {
        let _ = mode;
        if is_heif {
            return Err(DecodeError::Raw {
                path: path.to_path_buf(),
                message: "HEIC/HEIF decoding requires macOS CoreImage/ImageIO".to_owned(),
            });
        }
        decode_rawler_linear(path, max_edge)
    }
}

fn decode_raster_image(path: &Path, max_edge: Option<u32>) -> Result<LinearImage, DecodeError> {
    use image::{DynamicImage, ImageDecoder, ImageReader, metadata::Orientation};

    let reader = ImageReader::open(path)
        .map_err(|source| DecodeError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .with_guessed_format()
        .map_err(|source| DecodeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let mut decoder = reader.into_decoder().map_err(|source| DecodeError::Image {
        path: path.to_path_buf(),
        source,
    })?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut dyn_img = DynamicImage::from_decoder(decoder).map_err(|source| DecodeError::Image {
        path: path.to_path_buf(),
        source,
    })?;
    if orientation != Orientation::NoTransforms {
        dyn_img.apply_orientation(orientation);
    }

    let (orig_w, orig_h) = (dyn_img.width(), dyn_img.height());
    if orig_w == 0 || orig_h == 0 {
        return Err(DecodeError::EmptyDimensions(orig_w, orig_h));
    }

    if let Some(limit) = max_edge
        && orig_w.max(orig_h) > limit
    {
        dyn_img = dyn_img.resize(limit, limit, image::imageops::FilterType::Triangle);
    }

    let rgba32f = dyn_img.to_rgba32f();
    let (width, height) = rgba32f.dimensions();
    if width == 0 || height == 0 {
        return Err(DecodeError::EmptyDimensions(width, height));
    }

    let raw_samples = rgba32f.as_raw();
    let pixel_count = (width as usize) * (height as usize);
    let mut rgba16f = vec![f16::ZERO; pixel_count * 4];

    rgba16f
        .par_chunks_exact_mut(4)
        .zip(raw_samples.par_chunks_exact(4))
        .for_each(|(dst, src)| {
            dst[0] = f16::from_f32(srgb_to_linear(src[0]));
            dst[1] = f16::from_f32(srgb_to_linear(src[1]));
            dst[2] = f16::from_f32(srgb_to_linear(src[2]));
            dst[3] = f16::from_f32(src[3].clamp(0.0, 1.0));
        });

    let histogram_proxy = build_histogram_proxy(&rgba16f, width, height);
    let display_name = file_display_name(path);
    let ext = file_ext_upper(path);
    let exif = crate::exif::extract_exif_from_path(path);

    Ok(LinearImage {
        width,
        height,
        rgba16f,
        histogram_proxy,
        display_name,
        source_path: Some(path.to_path_buf()),
        format_label: format!("{ext} • 16-bit Linear FP16"),
        exif,
        camera_calibration: None,
    })
}

/// Extracts dual-illuminant [`CameraCalibration`] metadata (`make`, `model`, `D65` & `StdA`
/// color matrices, and normalized `as_shot_wb`) from a `rawler::RawImage`.
pub(crate) fn extract_camera_calibration_from_rawler(
    raw_img: &rawler::RawImage,
) -> Option<CameraCalibration> {
    use rawler::imgop::xyz::Illuminant;

    let make = if !raw_img.clean_make.trim().is_empty() {
        raw_img.clean_make.trim().to_owned()
    } else {
        raw_img.make.trim().to_owned()
    };
    let model = if !raw_img.clean_model.trim().is_empty() {
        raw_img.clean_model.trim().to_owned()
    } else {
        raw_img.model.trim().to_owned()
    };

    let to_mat9 = |flat: &[f32]| -> Option<[f32; 9]> {
        if flat.len() < 9 {
            return None;
        }
        let mut m = [0.0_f32; 9];
        m.copy_from_slice(&flat[..9]);
        if m.iter().all(|v| v.is_finite()) && m.iter().any(|v| v.abs() > 1e-6) {
            Some(m)
        } else {
            None
        }
    };

    let matrix_d65 = raw_img
        .color_matrix
        .get(&Illuminant::D65)
        .and_then(|v| to_mat9(v))
        .or_else(|| {
            raw_img
                .color_matrix
                .get(&Illuminant::D50)
                .and_then(|v| to_mat9(v))
        })
        .or_else(|| {
            raw_img
                .color_matrix
                .get(&Illuminant::Daylight)
                .and_then(|v| to_mat9(v))
        })
        .or_else(|| {
            let m = [
                raw_img.xyz_to_cam[0][0],
                raw_img.xyz_to_cam[0][1],
                raw_img.xyz_to_cam[0][2],
                raw_img.xyz_to_cam[1][0],
                raw_img.xyz_to_cam[1][1],
                raw_img.xyz_to_cam[1][2],
                raw_img.xyz_to_cam[2][0],
                raw_img.xyz_to_cam[2][1],
                raw_img.xyz_to_cam[2][2],
            ];
            to_mat9(&m)
        });

    let matrix_std_a = raw_img
        .color_matrix
        .get(&Illuminant::A)
        .and_then(|v| to_mat9(v))
        .or_else(|| {
            raw_img
                .color_matrix
                .get(&Illuminant::Tungsten)
                .and_then(|v| to_mat9(v))
        });

    let as_shot_wb = if raw_img.wb_coeffs[0].is_finite()
        && raw_img.wb_coeffs[1].is_finite()
        && raw_img.wb_coeffs[2].is_finite()
        && raw_img.wb_coeffs[0] > 0.0
        && raw_img.wb_coeffs[1] > 0.0
        && raw_img.wb_coeffs[2] > 0.0
    {
        let g = raw_img.wb_coeffs[1];
        Some([raw_img.wb_coeffs[0] / g, 1.0, raw_img.wb_coeffs[2] / g])
    } else {
        None
    };

    if make.is_empty()
        && model.is_empty()
        && matrix_d65.is_none()
        && matrix_std_a.is_none()
        && as_shot_wb.is_none()
    {
        return None;
    }

    Some(CameraCalibration {
        make,
        model,
        matrix_d65,
        matrix_std_a,
        as_shot_wb,
    })
}

/// Fast header-only RAW camera calibration probe via `rawler` dummy decode.
///
/// Returns `None` cleanly for non-RAW files or unrecognized containers.
pub fn extract_raw_camera_calibration(path: &Path) -> Option<CameraCalibration> {
    use rawler::decoders::RawDecodeParams;
    use rawler::rawsource::RawSource;

    if !is_raw_extension(path) || is_heif_extension(path) {
        return None;
    }
    let raw_source = RawSource::new(path).ok()?;
    let decoder = rawler::get_decoder(&raw_source).ok()?;
    let raw_img = decoder
        .raw_image(&raw_source, &RawDecodeParams::default(), true)
        .ok()?;
    extract_camera_calibration_from_rawler(&raw_img)
}

/// Tier 2 cross-platform RAW decoder built on pure-Rust `rawler`.
pub(crate) fn decode_rawler_linear(
    path: &Path,
    max_edge: Option<u32>,
) -> Result<LinearImage, DecodeError> {
    use rawler::decoders::RawDecodeParams;
    use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};
    use rawler::rawsource::RawSource;

    let raw_source = RawSource::new(path).map_err(|source| DecodeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let decoder = rawler::get_decoder(&raw_source).map_err(|e| DecodeError::Raw {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    let raw_img = decoder
        .raw_image(&raw_source, &RawDecodeParams::default(), false)
        .map_err(|e| DecodeError::Raw {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

    if raw_img.width == 0 || raw_img.height == 0 {
        return Err(DecodeError::EmptyDimensions(
            raw_img.width as u32,
            raw_img.height as u32,
        ));
    }

    // Develop to scene-referred linear RGB (deliberately excluding `ProcessingStep::SRgb`).
    // Avoid `CropActiveArea` if Fuji rotation is active (rawler requires rotation first).
    let mut steps = vec![ProcessingStep::Rescale, ProcessingStep::Demosaic];
    if raw_img.fuji_rotation_width.is_some() {
        steps.push(ProcessingStep::FujiRotate);
    } else {
        steps.push(ProcessingStep::CropActiveArea);
    }
    steps.extend_from_slice(&[
        ProcessingStep::WhiteBalance,
        ProcessingStep::Calibrate,
        ProcessingStep::CropDefault,
    ]);

    let developed = RawDevelop::new_with(&steps).develop_intermediate(&raw_img);

    let (mut width, mut height, mut linear_rgb): (u32, u32, Vec<[f32; 3]>) = match developed {
        Ok(Intermediate::ThreeColor(img)) => {
            let dim = img.dim();
            (dim.w as u32, dim.h as u32, img.data)
        }
        Ok(Intermediate::Monochrome(img)) => {
            let dim = img.dim();
            let rgb = img.data.into_iter().map(|v| [v, v, v]).collect();
            (dim.w as u32, dim.h as u32, rgb)
        }
        Ok(Intermediate::FourColor(img)) => {
            let dim = img.dim();
            let rgb = img
                .data
                .into_iter()
                .map(|p| [p[0], 0.5 * (p[1] + p[3]), p[2]])
                .collect();
            (dim.w as u32, dim.h as u32, rgb)
        }
        Err(_) => demosaic_bayer_bilinear_linear(&raw_img)?,
    };

    if width == 0 || height == 0 {
        return Err(DecodeError::EmptyDimensions(width, height));
    }

    // Apply EXIF orientation so the decoded RAW stands upright.
    let (ow, oh, oriented) = apply_orientation_rgb(width, height, &linear_rgb, raw_img.orientation);
    width = ow;
    height = oh;
    linear_rgb = oriented;

    // Optional proxy downscaling in linear light.
    if let Some(limit) = max_edge
        && width.max(height) > limit
    {
        let (dw, dh, scaled) = downsample_linear_rgb(&linear_rgb, width, height, limit);
        width = dw;
        height = dh;
        linear_rgb = scaled;
    }

    let mut rgba16f = vec![f16::ZERO; (width as usize) * (height as usize) * 4];
    rgba16f
        .par_chunks_exact_mut(4)
        .zip(linear_rgb.par_iter())
        .for_each(|(dst, &[r, g, b])| {
            dst[0] = f16::from_f32(sanitize_linear(r));
            dst[1] = f16::from_f32(sanitize_linear(g));
            dst[2] = f16::from_f32(sanitize_linear(b));
            dst[3] = f16::ONE;
        });

    let histogram_proxy = build_histogram_proxy(&rgba16f, width, height);
    let display_name = file_display_name(path);
    let ext = file_ext_upper(path);
    let exif = crate::exif::extract_exif_from_path(path);
    let camera_calibration = extract_camera_calibration_from_rawler(&raw_img);

    Ok(LinearImage {
        width,
        height,
        rgba16f,
        histogram_proxy,
        display_name,
        source_path: Some(path.to_path_buf()),
        format_label: format!("{ext} • RAW Linear FP16"),
        exif,
        camera_calibration,
    })
}

/// Bilinear Bayer/CFA demosaic + black-level & white-balance normalization fallback
/// for any RAW sensor buffer where full `RawDevelop` cannot run.
pub(crate) fn demosaic_bayer_bilinear_linear(
    raw: &rawler::RawImage,
) -> Result<(u32, u32, Vec<[f32; 3]>), DecodeError> {
    let w = raw.width;
    let h = raw.height;
    if w == 0 || h == 0 {
        return Err(DecodeError::EmptyDimensions(w as u32, h as u32));
    }

    let samples = raw.data.as_f32();
    let black = &raw.blacklevel.levels;
    let white = &raw.whitelevel.0;

    // Normalize white-balance multipliers so green sits near 1.0.
    let wb_g = if raw.wb_coeffs[1].is_finite() && raw.wb_coeffs[1] > 0.0 {
        raw.wb_coeffs[1]
    } else {
        1.0
    };
    let wb = [
        if raw.wb_coeffs[0].is_finite() && raw.wb_coeffs[0] > 0.0 {
            raw.wb_coeffs[0] / wb_g
        } else {
            1.0
        },
        1.0,
        if raw.wb_coeffs[2].is_finite() && raw.wb_coeffs[2] > 0.0 {
            raw.wb_coeffs[2] / wb_g
        } else {
            1.0
        },
    ];

    if raw.cpp >= 3 {
        let mut out = Vec::with_capacity(w * h);
        for px in samples.chunks_exact(raw.cpp) {
            let mut rgb = [0.0f32; 3];
            for c in 0..3 {
                let b = black.get(c).map(|r| r.as_f32()).unwrap_or(0.0);
                let wh = white.get(c).copied().unwrap_or(65535) as f32;
                let denom = (wh - b).max(1.0);
                rgb[c] = (((px[c] - b) / denom) * wb[c]).max(0.0);
            }
            out.push(rgb);
        }
        return Ok((w as u32, h as u32, out));
    }

    let cfa = match &raw.photometric {
        rawler::rawimage::RawPhotometricInterpretation::Cfa(cfg) => Some(cfg.cfa.clone()),
        _ => None,
    };

    let mut normalized = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            let color = cfa.as_ref().map(|c| c.color_at(y, x)).unwrap_or(0).min(3);
            let b = black
                .get(color)
                .or_else(|| black.first())
                .map(|r| r.as_f32())
                .unwrap_or(0.0);
            let wh = white
                .get(color)
                .or_else(|| white.first())
                .copied()
                .unwrap_or(65535) as f32;
            let denom = (wh - b).max(1.0);
            normalized[idx] = ((samples[idx] - b) / denom).max(0.0);
        }
    }

    Ok((
        w as u32,
        h as u32,
        bilinear_demosaic_plane(&normalized, w, h, cfa.as_ref(), wb),
    ))
}

/// Pure bilinear demosaic over a normalized 1-channel CFA plane (also directly unit-tested).
pub(crate) fn bilinear_demosaic_plane(
    plane: &[f32],
    w: usize,
    h: usize,
    cfa: Option<&rawler::CFA>,
    wb: [f32; 3],
) -> Vec<[f32; 3]> {
    let Some(cfa) = cfa else {
        return plane
            .iter()
            .map(|&v| [v * wb[0], v * wb[1], v * wb[2]])
            .collect();
    };

    let mut out = vec![[0.0f32; 3]; w * h];
    out.par_chunks_exact_mut(w)
        .enumerate()
        .for_each(|(y, row)| {
            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(h - 1);
            for (x, dst) in row.iter_mut().enumerate() {
                let x0 = x.saturating_sub(1);
                let x1 = (x + 1).min(w - 1);

                let mut sums = [0.0f32; 3];
                let mut counts = [0u32; 3];

                for ny in y0..=y1 {
                    for nx in x0..=x1 {
                        let c = cfa.color_at(ny, nx);
                        // Map Bayer R=0, G=1, B=2, E/G2=3 -> RGB indices 0..3.
                        let ch = match c {
                            0 => 0,
                            1 | 3 => 1,
                            2 => 2,
                            _ => 1,
                        };
                        sums[ch] += plane[ny * w + nx];
                        counts[ch] += 1;
                    }
                }

                let center_c = cfa.color_at(y, x);
                let center_ch = match center_c {
                    0 => 0,
                    1 | 3 => 1,
                    2 => 2,
                    _ => 1,
                };
                let center_val = plane[y * w + x];

                let mut rgb = [0.0f32; 3];
                for ch in 0..3 {
                    let v = if ch == center_ch {
                        center_val
                    } else if counts[ch] > 0 {
                        sums[ch] / counts[ch] as f32
                    } else {
                        center_val
                    };
                    rgb[ch] = (v * wb[ch]).max(0.0);
                }
                *dst = rgb;
            }
        });
    out
}

fn apply_orientation_rgb(
    width: u32,
    height: u32,
    pixels: &[[f32; 3]],
    orientation: rawler::Orientation,
) -> (u32, u32, Vec<[f32; 3]>) {
    use rawler::Orientation;

    if matches!(orientation, Orientation::Normal | Orientation::Unknown) {
        return (width, height, pixels.to_vec());
    }

    let w = width as usize;
    let h = height as usize;
    let (transpose, flip_x, flip_y) = orientation.to_flips();
    let (out_w, out_h) = if transpose { (h, w) } else { (w, h) };
    let mut out = vec![[0.0f32; 3]; out_w * out_h];

    for y in 0..h {
        let sy = if flip_y { h - 1 - y } else { y };
        for x in 0..w {
            let sx = if flip_x { w - 1 - x } else { x };
            let src = pixels[sy * w + sx];
            let (dx, dy) = if transpose { (y, x) } else { (x, y) };
            out[dy * out_w + dx] = src;
        }
    }

    (out_w as u32, out_h as u32, out)
}

/// Area-averaging linear-light downscaler preserving aspect ratio.
pub(crate) fn downsample_linear_rgb(
    src: &[[f32; 3]],
    src_w: u32,
    src_h: u32,
    max_edge: u32,
) -> (u32, u32, Vec<[f32; 3]>) {
    let (dst_w, dst_h) = fit_dimensions(src_w, src_h, max_edge);
    if dst_w == src_w && dst_h == src_h {
        return (src_w, src_h, src.to_vec());
    }

    let sw = src_w as usize;
    let sh = src_h as usize;
    let dw = dst_w as usize;
    let dh = dst_h as usize;
    let mut dst = vec![[0.0f32; 3]; dw * dh];

    dst.par_chunks_exact_mut(dw)
        .enumerate()
        .for_each(|(dy, row)| {
            let y0 = (dy * sh) / dh;
            let y1 = (((dy + 1) * sh) / dh).max(y0 + 1).min(sh);
            for (dx, pixel) in row.iter_mut().enumerate() {
                let x0 = (dx * sw) / dw;
                let x1 = (((dx + 1) * sw) / dw).max(x0 + 1).min(sw);

                let mut acc = [0.0f32; 3];
                let mut n = 0u32;
                for sy in y0..y1 {
                    let base = sy * sw;
                    for sx in x0..x1 {
                        let p = src[base + sx];
                        acc[0] += p[0];
                        acc[1] += p[1];
                        acc[2] += p[2];
                        n += 1;
                    }
                }
                let inv = 1.0 / n.max(1) as f32;
                *pixel = [acc[0] * inv, acc[1] * inv, acc[2] * inv];
            }
        });

    (dst_w, dst_h, dst)
}

pub(crate) fn fit_dimensions(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    let max_edge = max_edge.max(1);
    let longest = width.max(height);
    if longest <= max_edge || width == 0 || height == 0 {
        return (width.max(1), height.max(1));
    }
    let scale = max_edge as f64 / longest as f64;
    let w = ((width as f64 * scale).round() as u32).max(1);
    let h = ((height as f64 * scale).round() as u32).max(1);
    (w, h)
}

#[inline]
pub(crate) fn sanitize_linear(v: f32) -> f32 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

pub(crate) fn file_display_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_owned())
}

pub(crate) fn file_ext_upper(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_uppercase())
        .unwrap_or_else(|| "IMAGE".to_owned())
}

#[cfg(target_os = "macos")]
pub(crate) mod macos {
    use std::ffi::c_void;
    use std::path::Path;
    use std::ptr::NonNull;
    use std::sync::OnceLock;

    use half::f16;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::AnyObject;
    use objc2::sel;
    use objc2_core_foundation::{
        CFDictionary, CFNumber, CFString, CFType, CFURL, CGAffineTransform, CGPoint, CGRect, CGSize,
    };
    use objc2_core_graphics::{CGColorSpace, kCGColorSpaceExtendedLinearSRGB};
    use objc2_core_image::{
        CIContext, CIImage, CIRAWFilter, kCIFormatRGBAh, kCIImageApplyOrientationProperty,
    };
    use objc2_foundation::{NSDictionary, NSNumber, NSObjectProtocol, NSString, NSURL};
    use objc2_image_io::{
        CGImageSource, kCGImagePropertyOrientation, kCGImagePropertyPixelHeight,
        kCGImagePropertyPixelWidth, kCGImagePropertyTIFFDictionary,
    };

    use super::{
        RawDecodeMode, extract_raw_camera_calibration, file_display_name, file_ext_upper,
        sanitize_linear,
    };
    use crate::{DecodeError, LinearImage, build_histogram_proxy};

    /// Maximum single-axis dimension accepted from CoreImage to prevent runaway allocations.
    const MAX_RENDER_DIM: usize = 32_768;

    /// Thread-safe wrapper around Apple's `CIContext`.
    struct SharedCIContext(Retained<CIContext>);

    // SAFETY: Apple documents `CIContext` as immutable and thread-safe for concurrent
    // `render:toBitmap:rowBytes:bounds:format:colorSpace:` invocations across threads.
    unsafe impl Send for SharedCIContext {}
    unsafe impl Sync for SharedCIContext {}

    fn shared_ci_context() -> &'static CIContext {
        static CTX: OnceLock<SharedCIContext> = OnceLock::new();
        &CTX.get_or_init(|| SharedCIContext(unsafe { CIContext::context() }))
            .0
    }

    /// Header-only `<1ms` dimension probe via `ImageIO::CGImageSourceCopyPropertiesAtIndex`,
    /// respecting EXIF orientation transpose (`5..=8`).
    pub(crate) fn probe_imageio_dimensions(path: &Path) -> Result<[u32; 2], DecodeError> {
        let abs_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        };

        autoreleasepool(|_| {
            let cf_url = CFURL::from_file_path(&abs_path).ok_or_else(|| DecodeError::Raw {
                path: path.to_path_buf(),
                message: "Failed to construct CFURL for dimension probe".to_owned(),
            })?;
            let source = unsafe { CGImageSource::with_url(&cf_url, None) }.ok_or_else(|| {
                DecodeError::Raw {
                    path: path.to_path_buf(),
                    message: "CGImageSourceCreateWithURL returned null".to_owned(),
                }
            })?;
            let props =
                unsafe { source.properties_at_index(0, None) }.ok_or_else(|| DecodeError::Raw {
                    path: path.to_path_buf(),
                    message: "CGImageSourceCopyPropertiesAtIndex returned null".to_owned(),
                })?;

            let w = dict_get_i64(&props, unsafe { kCGImagePropertyPixelWidth }).unwrap_or(0);
            let h = dict_get_i64(&props, unsafe { kCGImagePropertyPixelHeight }).unwrap_or(0);
            if w <= 0 || h <= 0 || w > u32::MAX as i64 || h > u32::MAX as i64 {
                return Err(DecodeError::EmptyDimensions(
                    w.max(0) as u32,
                    h.max(0) as u32,
                ));
            }

            let orientation = dict_get_i64(&props, unsafe { kCGImagePropertyOrientation })
                .or_else(|| {
                    let tiff = dict_get_raw(&props, unsafe { kCGImagePropertyTIFFDictionary })?
                        .downcast_ref::<CFDictionary>()?;
                    let key = CFString::from_static_str("Orientation");
                    dict_get_i64(tiff, &key)
                })
                .unwrap_or(1);

            if (5..=8).contains(&orientation) {
                Ok([h as u32, w as u32])
            } else {
                Ok([w as u32, h as u32])
            }
        })
    }

    fn dict_get_raw<'a>(dict: &'a CFDictionary, key: &CFString) -> Option<&'a CFType> {
        let key_ptr: *const CFString = key;
        let val = unsafe { dict.value(key_ptr.cast::<c_void>()) };
        if val.is_null() {
            None
        } else {
            // SAFETY: ImageIO property dictionaries hold CFType values borrowed for `dict`'s lifetime.
            Some(unsafe { &*val.cast::<CFType>() })
        }
    }

    fn dict_get_i64(dict: &CFDictionary, key: &CFString) -> Option<i64> {
        dict_get_raw(dict, key)?
            .downcast_ref::<CFNumber>()?
            .as_i64()
    }

    /// Decodes a RAW, ProRAW, HEIC, or raster file into scene-referred linear `Rgba16Float`
    /// using Apple's native `CIRAWFilter` / `CoreImage` pipeline in true linear mode.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn decode_coreimage_linear(
        path: &Path,
        max_edge: Option<u32>,
        try_raw_filter: bool,
    ) -> Result<LinearImage, DecodeError> {
        decode_coreimage_linear_with_mode(path, max_edge, try_raw_filter, RawDecodeMode::Linear)
    }

    /// Decodes a RAW, ProRAW, HEIC, or raster file into scene-referred linear `Rgba16Float`
    /// using Apple's native `CIRAWFilter` / `CoreImage` pipeline with explicit [`RawDecodeMode`].
    pub(crate) fn decode_coreimage_linear_with_mode(
        path: &Path,
        max_edge: Option<u32>,
        try_raw_filter: bool,
        mode: RawDecodeMode,
    ) -> Result<LinearImage, DecodeError> {
        let abs_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        };

        let path_str = abs_path.to_str().ok_or_else(|| DecodeError::Raw {
            path: path.to_path_buf(),
            message: "File path is not valid UTF-8".to_owned(),
        })?;

        autoreleasepool(|_| {
            let ns_path = NSString::from_str(path_str);
            let ns_url = NSURL::fileURLWithPath(&ns_path);

            let (ci_image, used_raw_filter): (Retained<CIImage>, bool) = if try_raw_filter {
                if let Some(raw_filter) = unsafe { CIRAWFilter::filterWithImageURL(&ns_url) } {
                    unsafe {
                        // Preserve highlight headroom and enable Apple highlight recovery when supported
                        // (runtime selector guard required because highlightRecovery APIs are macOS 26+).
                        raw_filter.setExtendedDynamicRangeAmount(1.0);
                        if raw_filter.respondsToSelector(sel!(isHighlightRecoverySupported))
                            && raw_filter.respondsToSelector(sel!(setHighlightRecoveryEnabled:))
                            && raw_filter.isHighlightRecoverySupported()
                        {
                            raw_filter.setHighlightRecoveryEnabled(true);
                        }
                        if mode == RawDecodeMode::Linear {
                            // Disable Apple's baked tone boost, local tone mapping, and contrast so
                            // Rune's linear Base Characteristics FilmCurve and 3D profile LUT never
                            // double-apply a tone curve on macOS RAW files.
                            raw_filter.setBoostAmount(0.0);
                            raw_filter.setBoostShadowAmount(0.0);
                            if raw_filter.isLocalToneMapSupported() {
                                raw_filter.setLocalToneMapAmount(0.0);
                            }
                            if raw_filter.isContrastSupported() {
                                raw_filter.setContrastAmount(0.0);
                            }
                        }
                        if let Some(limit) = max_edge {
                            let native = raw_filter.nativeSize();
                            let longest = native.width.max(native.height);
                            if longest > limit as f64 && longest.is_finite() {
                                let factor = (limit as f64 / longest).clamp(0.01, 1.0) as f32;
                                raw_filter.setScaleFactor(factor);
                            }
                        }
                    }
                    let output =
                        unsafe { raw_filter.outputImage() }.ok_or_else(|| DecodeError::Raw {
                            path: path.to_path_buf(),
                            message: "CIRAWFilter produced no outputImage".to_owned(),
                        })?;
                    (output, true)
                } else {
                    let img = load_ciimage_oriented(&ns_url, path)?;
                    (img, false)
                }
            } else {
                let img = load_ciimage_oriented(&ns_url, path)?;
                (img, false)
            };

            let mut extent = unsafe { ci_image.extent() };
            if !extent.size.width.is_finite()
                || !extent.size.height.is_finite()
                || extent.size.width < 1.0
                || extent.size.height < 1.0
            {
                return Err(DecodeError::EmptyDimensions(0, 0));
            }

            // Apply scale transform when max_edge wasn't already satisfied by CIRAWFilter::scaleFactor.
            let ci_image = if let Some(limit) = max_edge {
                let longest = extent.size.width.max(extent.size.height);
                if longest > (limit as f64 + 0.5) {
                    let scale = (limit as f64 / longest).clamp(0.001, 1.0);
                    let transform = CGAffineTransform {
                        a: scale,
                        b: 0.0,
                        c: 0.0,
                        d: scale,
                        tx: 0.0,
                        ty: 0.0,
                    };
                    let scaled = unsafe { ci_image.imageByApplyingTransform(transform) };
                    extent = unsafe { scaled.extent() };
                    scaled
                } else {
                    ci_image
                }
            } else {
                ci_image
            };

            let width = (extent.size.width.round() as usize).clamp(1, MAX_RENDER_DIM);
            let height = (extent.size.height.round() as usize).clamp(1, MAX_RENDER_DIM);
            let render_bounds = CGRect {
                origin: CGPoint {
                    x: extent.origin.x.floor(),
                    y: extent.origin.y.floor(),
                },
                size: CGSize {
                    width: width as f64,
                    height: height as f64,
                },
            };

            let color_space =
                CGColorSpace::with_name(Some(unsafe { kCGColorSpaceExtendedLinearSRGB }))
                    .ok_or_else(|| DecodeError::Raw {
                        path: path.to_path_buf(),
                        message: "Failed to create kCGColorSpaceExtendedLinearSRGB".to_owned(),
                    })?;

            let pixel_count = width
                .checked_mul(height)
                .ok_or_else(|| DecodeError::InvalidBuffer("Image dimension overflow".to_owned()))?;
            let elem_count = pixel_count.checked_mul(4).ok_or_else(|| {
                DecodeError::InvalidBuffer("Image buffer size overflow".to_owned())
            })?;
            let row_bytes = width
                .checked_mul(8)
                .and_then(|v| isize::try_from(v).ok())
                .ok_or_else(|| DecodeError::InvalidBuffer("Image stride overflow".to_owned()))?;

            // `f16` is IEEE 754 binary16 (2 bytes); 4 channels = 8 bytes/pixel matching `kCIFormatRGBAh`.
            let mut rgba16f = vec![f16::ZERO; elem_count];
            let data_ptr =
                NonNull::new(rgba16f.as_mut_ptr().cast::<c_void>()).ok_or_else(|| {
                    DecodeError::InvalidBuffer("Null destination buffer pointer".to_owned())
                })?;

            let ci_context = shared_ci_context();
            unsafe {
                ci_context.render_toBitmap_rowBytes_bounds_format_colorSpace(
                    &ci_image,
                    data_ptr,
                    row_bytes,
                    render_bounds,
                    kCIFormatRGBAh,
                    Some(&color_space),
                );
            }

            // Sanitize any negative/NaN sensor excursions while preserving >1.0 HDR headroom.
            for px in rgba16f.as_chunks_mut::<4>().0 {
                px[0] = f16::from_f32(sanitize_linear(px[0].to_f32()));
                px[1] = f16::from_f32(sanitize_linear(px[1].to_f32()));
                px[2] = f16::from_f32(sanitize_linear(px[2].to_f32()));
                let a = px[3].to_f32();
                px[3] = f16::from_f32(if a.is_finite() {
                    a.clamp(0.0, 1.0)
                } else {
                    1.0
                });
            }

            let w32 = width as u32;
            let h32 = height as u32;
            let histogram_proxy = build_histogram_proxy(&rgba16f, w32, h32);
            let display_name = file_display_name(path);
            let ext = file_ext_upper(path);
            let tier = if used_raw_filter {
                "Apple RAW FP16"
            } else {
                "CoreImage Linear FP16"
            };
            let exif = crate::exif::extract_exif_from_path(path);
            let camera_calibration = if used_raw_filter {
                extract_raw_camera_calibration(path)
            } else {
                None
            };

            Ok(LinearImage {
                width: w32,
                height: h32,
                rgba16f,
                histogram_proxy,
                display_name,
                source_path: Some(path.to_path_buf()),
                format_label: format!("{ext} • {tier}"),
                exif,
                camera_calibration,
            })
        })
    }

    fn load_ciimage_oriented(
        ns_url: &NSURL,
        path: &Path,
    ) -> Result<Retained<CIImage>, DecodeError> {
        let yes = NSNumber::new_bool(true);
        let yes_obj: &AnyObject = yes.as_ref();
        let key = unsafe { kCIImageApplyOrientationProperty };
        let options = NSDictionary::from_slices(&[key], &[yes_obj]);
        unsafe { CIImage::imageWithContentsOfURL_options(ns_url, Some(&options)) }.ok_or_else(
            || DecodeError::Raw {
                path: path.to_path_buf(),
                message: "CoreImage could not open image URL".to_owned(),
            },
        )
    }
}
