//! Folder scanning and ultra-fast filmstrip thumbnail generation.
//!
//! On macOS, `load_thumbnail` uses `ImageIO`'s `CGImageSourceCreateThumbnailAtIndex`,
//! extracting embedded EXIF thumbnails in <3ms even on 50MP Camera RAW files with
//! automatic EXIF orientation correction. On other platforms (or if ImageIO fails),
//! it falls back cleanly to `rawler` embedded previews or `image::open` + fast resize.

use std::path::{Path, PathBuf};

use rune_editor_core::{DevelopRecipe, linear_to_srgb};

use crate::raw::{fit_dimensions, is_raw_extension, is_supported_photo};
use crate::{DecodeError, LinearImage};

/// Ready-to-upload 8-bit sRGB RGBA thumbnail bitmap for the filmstrip UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbnailRgba8 {
    pub width: u32,
    pub height: u32,
    /// Tightly-packed `RGBA8` pixels (`width * height * 4` bytes, sRGB).
    pub rgba: Vec<u8>,
}

/// Scans `dir` (non-recursively) and returns all supported photo paths (RAW + raster),
/// skipping hidden dotfiles (including macOS `._*` AppleDouble files) and sorting
/// deterministically by filename (case-insensitive, with exact path tiebreaker).
pub fn scan_photo_folder(dir: &Path) -> Result<Vec<PathBuf>, DecodeError> {
    let entries = std::fs::read_dir(dir).map_err(|source| DecodeError::Io {
        path: dir.to_path_buf(),
        source,
    })?;

    let mut photos = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();

        // Skip hidden files and macOS AppleDouble resource forks (`.DS_Store`, `._IMG_001.DNG`).
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && name.starts_with('.')
        {
            continue;
        }

        if path.is_file() && is_supported_photo(&path) {
            photos.push(path);
        }
    }

    photos.sort_by(|a, b| {
        let a_name = a
            .file_name()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let b_name = b
            .file_name()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        a_name.cmp(&b_name).then_with(|| a.cmp(b))
    });

    Ok(photos)
}

/// Loads a fast filmstrip thumbnail whose longest edge is at most `max_edge` pixels.
///
/// - **macOS fast path:** `ImageIO` `CGImageSourceCreateThumbnailAtIndex` with
///   `kCGImageSourceCreateThumbnailFromImageIfAbsent` and `kCGImageSourceCreateThumbnailWithTransform`.
/// - **Fallback path:** `rawler` embedded thumbnail/preview for RAW files, or `image::open`
///   with fast bilinear resizing.
pub fn load_thumbnail(path: &Path, max_edge: u32) -> Result<ThumbnailRgba8, DecodeError> {
    let max_edge = max_edge.max(1);

    #[cfg(target_os = "macos")]
    {
        if let Ok(thumb) = macos::load_imageio_thumbnail(path, max_edge) {
            return Ok(thumb);
        }
    }

    if is_raw_extension(path)
        && let Ok(thumb) = load_rawler_thumbnail(path, max_edge)
    {
        return Ok(thumb);
    }

    load_raster_thumbnail(path, max_edge)
}

/// Generates an 8-bit sRGB filmstrip thumbnail from an in-memory `LinearImage`
/// (used for the built-in HDR demo session and in-memory previews).
pub fn generate_demo_thumbnail(img: &LinearImage, max_edge: u32) -> ThumbnailRgba8 {
    let max_edge = max_edge.max(1);
    if img.width == 0
        || img.height == 0
        || img.rgba16f.len() < (img.width as usize * img.height as usize * 4)
    {
        return ThumbnailRgba8 {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 255],
        };
    }

    let (dst_w, dst_h) = fit_dimensions(img.width, img.height, max_edge);
    let sw = img.width as usize;
    let sh = img.height as usize;
    let dw = dst_w as usize;
    let dh = dst_h as usize;

    let compiled_neutral = DevelopRecipe::default().compile();
    let mut rgba = vec![0u8; dw * dh * 4];

    for dy in 0..dh {
        let y0 = (dy * sh) / dh;
        let y1 = (((dy + 1) * sh) / dh).max(y0 + 1).min(sh);
        for dx in 0..dw {
            let x0 = (dx * sw) / dw;
            let x1 = (((dx + 1) * sw) / dw).max(x0 + 1).min(sw);

            let mut acc = [0.0f32; 3];
            let mut count = 0u32;
            for sy in y0..y1 {
                let row_base = sy * sw;
                for sx in x0..x1 {
                    let idx = (row_base + sx) * 4;
                    acc[0] += img.rgba16f[idx].to_f32();
                    acc[1] += img.rgba16f[idx + 1].to_f32();
                    acc[2] += img.rgba16f[idx + 2].to_f32();
                    count += 1;
                }
            }
            let inv = 1.0 / count.max(1) as f32;
            let lin =
                compiled_neutral.evaluate_pixel_linear([acc[0] * inv, acc[1] * inv, acc[2] * inv]);

            let out_idx = (dy * dw + dx) * 4;
            rgba[out_idx] = (linear_to_srgb(lin[0]) * 255.0).round().clamp(0.0, 255.0) as u8;
            rgba[out_idx + 1] = (linear_to_srgb(lin[1]) * 255.0).round().clamp(0.0, 255.0) as u8;
            rgba[out_idx + 2] = (linear_to_srgb(lin[2]) * 255.0).round().clamp(0.0, 255.0) as u8;
            rgba[out_idx + 3] = 255;
        }
    }

    ThumbnailRgba8 {
        width: dst_w,
        height: dst_h,
        rgba,
    }
}

fn load_raster_thumbnail(path: &Path, max_edge: u32) -> Result<ThumbnailRgba8, DecodeError> {
    let dyn_img = image::open(path).map_err(|source| DecodeError::Image {
        path: path.to_path_buf(),
        source,
    })?;
    dynamic_image_to_thumbnail(dyn_img, max_edge)
}

fn load_rawler_thumbnail(path: &Path, max_edge: u32) -> Result<ThumbnailRgba8, DecodeError> {
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
    let params = RawDecodeParams::default();

    // Prefer embedded EXIF thumbnail -> embedded JPEG preview -> full embedded JPEG.
    if let Ok(Some(img)) = decoder.thumbnail_image(&raw_source, &params) {
        return dynamic_image_to_thumbnail(img, max_edge);
    }
    if let Ok(Some(img)) = decoder.preview_image(&raw_source, &params) {
        return dynamic_image_to_thumbnail(img, max_edge);
    }
    if let Ok(Some(img)) = decoder.full_image(&raw_source, &params) {
        return dynamic_image_to_thumbnail(img, max_edge);
    }

    // Last resort: decode a small linear proxy via rawler and tonemap into a thumbnail.
    let linear = crate::raw::decode_rawler_linear(path, Some(max_edge))?;
    Ok(generate_demo_thumbnail(&linear, max_edge))
}

fn dynamic_image_to_thumbnail(
    dyn_img: image::DynamicImage,
    max_edge: u32,
) -> Result<ThumbnailRgba8, DecodeError> {
    let (w, h) = (dyn_img.width(), dyn_img.height());
    if w == 0 || h == 0 {
        return Err(DecodeError::EmptyDimensions(w, h));
    }

    let resized = if w.max(h) > max_edge {
        dyn_img.resize(max_edge, max_edge, image::imageops::FilterType::Triangle)
    } else {
        dyn_img
    };
    let rgba8 = resized.to_rgba8();
    let (tw, th) = rgba8.dimensions();

    Ok(ThumbnailRgba8 {
        width: tw,
        height: th,
        rgba: rgba8.into_raw(),
    })
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;
    use std::path::Path;

    use objc2_core_foundation::{
        CFBoolean, CFDictionary, CFNumber, CFString, CFType, CFURL, CGPoint, CGRect, CGSize,
    };
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
        kCGColorSpaceSRGB,
    };
    use objc2_image_io::{
        CGImageSource, kCGImageSourceCreateThumbnailFromImageIfAbsent,
        kCGImageSourceCreateThumbnailWithTransform, kCGImageSourceShouldCacheImmediately,
        kCGImageSourceThumbnailMaxPixelSize,
    };

    use super::ThumbnailRgba8;
    use crate::DecodeError;

    pub(super) fn load_imageio_thumbnail(
        path: &Path,
        max_edge: u32,
    ) -> Result<ThumbnailRgba8, DecodeError> {
        let abs_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        };

        let cf_url = CFURL::from_file_path(&abs_path).ok_or_else(|| DecodeError::Raw {
            path: path.to_path_buf(),
            message: "Failed to construct CFURL for thumbnail".to_owned(),
        })?;

        let source =
            unsafe { CGImageSource::with_url(&cf_url, None) }.ok_or_else(|| DecodeError::Raw {
                path: path.to_path_buf(),
                message: "CGImageSourceCreateWithURL returned null".to_owned(),
            })?;

        let max_size_num = CFNumber::new_i32(max_edge.min(i32::MAX as u32) as i32);
        let cf_true = CFBoolean::new(true);

        let keys: [&CFString; 4] = unsafe {
            [
                kCGImageSourceCreateThumbnailFromImageIfAbsent,
                kCGImageSourceCreateThumbnailWithTransform,
                kCGImageSourceThumbnailMaxPixelSize,
                kCGImageSourceShouldCacheImmediately,
            ]
        };
        let vals: [&CFType; 4] = [
            cf_true.as_ref(),
            cf_true.as_ref(),
            max_size_num.as_ref(),
            cf_true.as_ref(),
        ];
        let options = CFDictionary::<CFString, CFType>::from_slices(&keys, &vals);

        let cg_image = unsafe { source.thumbnail_at_index(0, Some(options.as_opaque())) }
            .ok_or_else(|| DecodeError::Raw {
                path: path.to_path_buf(),
                message: "CGImageSourceCreateThumbnailAtIndex returned null".to_owned(),
            })?;

        let width = CGImage::width(Some(&cg_image));
        let height = CGImage::height(Some(&cg_image));
        if width == 0 || height == 0 {
            return Err(DecodeError::EmptyDimensions(width as u32, height as u32));
        }

        let color_space =
            CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).ok_or_else(|| {
                DecodeError::Raw {
                    path: path.to_path_buf(),
                    message: "Failed to create kCGColorSpaceSRGB".to_owned(),
                }
            })?;

        let bytes_per_row = width
            .checked_mul(4)
            .ok_or_else(|| DecodeError::InvalidBuffer("Thumbnail stride overflow".to_owned()))?;
        let byte_len = bytes_per_row
            .checked_mul(height)
            .ok_or_else(|| DecodeError::InvalidBuffer("Thumbnail buffer overflow".to_owned()))?;

        let mut rgba = vec![0u8; byte_len];
        // PremultipliedLast with default byte order writes [R, G, B, A] bytes in order.
        let bitmap_info = CGImageAlphaInfo::PremultipliedLast.0;

        let ctx = unsafe {
            CGBitmapContextCreate(
                rgba.as_mut_ptr().cast::<c_void>(),
                width,
                height,
                8,
                bytes_per_row,
                Some(&color_space),
                bitmap_info,
            )
        }
        .ok_or_else(|| DecodeError::Raw {
            path: path.to_path_buf(),
            message: "CGBitmapContextCreate failed".to_owned(),
        })?;

        let rect = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: width as f64,
                height: height as f64,
            },
        };
        CGContext::draw_image(Some(&ctx), rect, Some(&cg_image));

        Ok(ThumbnailRgba8 {
            width: width as u32,
            height: height as u32,
            rgba,
        })
    }
}
