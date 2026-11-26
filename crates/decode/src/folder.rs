//! Folder scanning and ultra-fast filmstrip thumbnail generation.
//!
//! On macOS, `load_thumbnail` uses `ImageIO`'s `CGImageSourceCreateThumbnailAtIndex`,
//! extracting embedded EXIF thumbnails in <3ms even on 50MP Camera RAW files with
//! automatic EXIF orientation correction. On other platforms (or if ImageIO fails),
//! it falls back cleanly to `rawler` embedded previews or `image::open` + fast resize.

use std::collections::{HashMap, HashSet};
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

/// Options for [`scan_photo_folder_with_options`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanOptions {
    /// When `true`, recursively traverses subdirectories (skipping hidden dotfiles/dirs).
    pub recursive: bool,
    /// Optional lowercase extension whitelist (e.g. `["arw", "dng"]`); empty = all supported.
    pub extensions: Vec<String>,
}

/// Scans `dir` (non-recursively) and returns all supported photo paths (RAW + raster),
/// skipping hidden dotfiles (including macOS `._*` AppleDouble files) and sorting
/// deterministically by filename (case-insensitive, with exact path tiebreaker).
pub fn scan_photo_folder(dir: &Path) -> Result<Vec<PathBuf>, DecodeError> {
    scan_photo_folder_with_options(dir, &ScanOptions::default())
}

/// Zero-dependency polling watcher for Capture One-style Hot Folder auto-ingest.
///
/// Tracks file byte lengths across consecutive [`Self::poll_ready`] calls and only emits
/// newly arrived, non-empty regular image files once their size stabilizes across two
/// consecutive polls (preventing half-written camera/tether writes from decoding prematurely).
/// Skips symlinks and hidden dotfiles/AppleDouble files.
#[derive(Clone, Debug)]
pub struct HotFolderWatcher {
    watch_dir: PathBuf,
    known_files: HashSet<PathBuf>,
    pending_sizes: HashMap<PathBuf, u64>,
}

impl HotFolderWatcher {
    /// Creates a watcher over `dir`, seeding `known_files` with all existing top-level
    /// supported photo files so only *new* arrivals trigger [`Self::poll_ready`].
    pub fn new(dir: PathBuf) -> Result<Self, DecodeError> {
        let mut watcher = Self::new_empty(dir)?;
        for (path, _) in scan_top_level_regular_photos(&watcher.watch_dir)? {
            watcher.known_files.insert(path);
        }
        Ok(watcher)
    }

    /// Creates a watcher over `dir` starting with an empty `known_files` set (useful for
    /// deterministic `--once` ingest or fresh session capture directories).
    pub fn new_empty(dir: PathBuf) -> Result<Self, DecodeError> {
        let meta = std::fs::metadata(&dir).map_err(|source| DecodeError::Io {
            path: dir.clone(),
            source,
        })?;
        if !meta.is_dir() {
            return Err(DecodeError::InvalidPath(format!(
                "Hot folder path '{}' is not a directory",
                dir.display()
            )));
        }
        Ok(Self {
            watch_dir: dir,
            known_files: HashSet::new(),
            pending_sizes: HashMap::new(),
        })
    }

    pub fn watch_dir(&self) -> &Path {
        &self.watch_dir
    }

    pub fn known_count(&self) -> usize {
        self.known_files.len()
    }

    /// Polls `watch_dir` once and returns newly arrived regular photo files whose byte size
    /// is non-zero and stable across two consecutive polls, sorted deterministically.
    pub fn poll_ready(&mut self) -> Result<Vec<PathBuf>, DecodeError> {
        let current = scan_top_level_regular_photos(&self.watch_dir)?;
        let mut seen_this_poll = HashSet::with_capacity(current.len());
        let mut ready = Vec::new();

        for (path, size) in current {
            seen_this_poll.insert(path.clone());
            if self.known_files.contains(&path) {
                continue;
            }
            if size == 0 {
                self.pending_sizes.insert(path, 0);
                continue;
            }
            if self.pending_sizes.get(&path).copied() == Some(size) {
                self.pending_sizes.remove(&path);
                self.known_files.insert(path.clone());
                ready.push(path);
            } else {
                self.pending_sizes.insert(path, size);
            }
        }

        self.pending_sizes.retain(|p, _| seen_this_poll.contains(p));
        ready.sort_by(|a, b| {
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
        Ok(ready)
    }
}

fn scan_top_level_regular_photos(dir: &Path) -> Result<Vec<(PathBuf, u64)>, DecodeError> {
    let entries = std::fs::read_dir(dir).map_err(|source| DecodeError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    let mut out = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && name.starts_with('.')
        {
            continue;
        }
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        // Strictly skip symlinks and directories to prevent symlink escapes/cycles.
        if ft.is_symlink() || !ft.is_file() {
            continue;
        }
        if !is_supported_photo(&path) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        out.push((path, meta.len()));
    }
    Ok(out)
}

/// Maximum directory recursion depth during recursive folder scans (protects against deep trees/cycles).
pub const MAX_SCAN_DEPTH: usize = 32;

/// Scans `dir` according to [`ScanOptions`] and returns deterministically sorted photo paths.
pub fn scan_photo_folder_with_options(
    dir: &Path,
    opts: &ScanOptions,
) -> Result<Vec<PathBuf>, DecodeError> {
    let mut photos = Vec::new();
    let allowed: Vec<String> = opts
        .extensions
        .iter()
        .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .collect();
    collect_photos(dir, opts.recursive, &allowed, 0, &mut photos)?;

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

fn collect_photos(
    dir: &Path,
    recursive: bool,
    allowed_exts: &[String],
    depth: usize,
    out: &mut Vec<PathBuf>,
) -> Result<(), DecodeError> {
    if depth > MAX_SCAN_DEPTH {
        return Ok(());
    }

    let entries = std::fs::read_dir(dir).map_err(|source| DecodeError::Io {
        path: dir.to_path_buf(),
        source,
    })?;

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();

        // Skip hidden files/dirs and macOS AppleDouble resource forks (`.DS_Store`, `._IMG_001.DNG`).
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && name.starts_with('.')
        {
            continue;
        }

        let Ok(ft) = entry.file_type() else {
            continue;
        };

        // Never recurse into directory symlinks (prevents symlink cycle crashes / infinite recursion).
        if ft.is_symlink() {
            if !path.is_file() {
                continue;
            }
        } else if ft.is_dir() {
            if recursive && depth < MAX_SCAN_DEPTH {
                let _ = collect_photos(&path, true, allowed_exts, depth + 1, out);
            }
            continue;
        } else if !ft.is_file() {
            continue;
        }

        if is_supported_photo(&path) {
            if !allowed_exts.is_empty() {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .unwrap_or_default();
                if !allowed_exts.iter().any(|a| a == &ext) {
                    continue;
                }
            }
            out.push(path);
        }
    }
    Ok(())
}

/// Loads a fast detail proxy (`max_edge`, typically `1024px`) and converts it to
/// perceptual luma `sqrt(luma709(linear_rgb))` for sharpness & focus-mask analysis.
pub fn extract_detail_luma_proxy(
    path: &Path,
    max_edge: u32,
) -> Result<(Vec<f32>, u32, u32), DecodeError> {
    let thumb = load_thumbnail(path, max_edge.max(64))?;
    let px_count = (thumb.width as usize) * (thumb.height as usize);
    let mut luma = Vec::with_capacity(px_count);
    for chunk in thumb.rgba.as_chunks::<4>().0 {
        let r = rune_editor_core::srgb_to_linear(chunk[0] as f32 / 255.0);
        let g = rune_editor_core::srgb_to_linear(chunk[1] as f32 / 255.0);
        let b = rune_editor_core::srgb_to_linear(chunk[2] as f32 / 255.0);
        let y = (0.2126 * r + 0.7152 * g + 0.0722 * b).max(0.0);
        luma.push(y.sqrt());
    }
    Ok((luma, thumb.width, thumb.height))
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
