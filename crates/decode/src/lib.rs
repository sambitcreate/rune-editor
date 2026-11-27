//! Image decoding, Camera RAW / ProRAW / HEIC ingestion, folder scanning,
//! fast filmstrip thumbnails, and full-resolution photo export.
//!
//! Converts RAW and standard photo formats into 16-bit half-float scene-referred
//! linear `Rgba16Float` buffers ready for direct upload into the GPU develop
//! pipeline, plus downsampled proxies for sub-millisecond live histogram evaluation.

pub mod exif;
pub mod export;
pub mod folder;
pub mod model_manager;
pub mod raw;
pub mod sam_ort;
pub mod vision;
#[cfg(target_os = "macos")]
pub mod vision_apple;

use std::path::{Path, PathBuf};

use half::f16;
use rune_editor_core::CameraCalibration;
use thiserror::Error;

pub use exif::{PhotoExif, extract_exif_from_bytes, extract_exif_from_path, format_shutter_speed};
pub use export::{
    ExportColorSpace, ExportFormat, ExportOptions, ExportRecipeSpec, MultiRecipeExportItem,
    NamingContext, TextWatermark, WatermarkPosition, builtin_export_recipes,
    expand_naming_template, export_image, export_image_to_jpeg_vec, export_image_with_options,
    export_multi_recipe, guard_not_source, linear_srgb_to_display_p3, plan_multi_recipe_paths,
    sample_linear_rgb_at_uv, sample_raw_linear_rgb_at_uv, sanitize_filename_stem,
};
pub use folder::{
    HotFolderWatcher, MAX_SCAN_DEPTH, ScanOptions, ThumbnailRgba8, extract_detail_luma_proxy,
    generate_demo_thumbnail, load_thumbnail, scan_photo_folder, scan_photo_folder_with_options,
};
pub use model_manager::{
    ActiveEngineSummary, ModelDownloadProgress, ModelError, active_engine_summary, download_model,
    list_model_statuses, remove_model, setup_model_from_local_dir,
};
pub use raw::{
    RASTER_EXTENSIONS, RAW_EXTENSIONS, RawDecodeMode, SUPPORTED_EXTENSIONS,
    extract_raw_camera_calibration, is_raw_extension, is_supported_photo, load_file_proxy,
    load_file_proxy_with_mode, probe_image_dimensions,
};
pub use sam_ort::{SamMaskResult, evict_model_from_cache, evict_session_cache};
pub use vision::{
    SmartMaskResult, VisionError, generate_heuristic_mask, generate_interactive_heuristic_mask,
    generate_smart_mask, generate_subject_mask,
};
#[cfg(target_os = "macos")]
pub use vision_apple::VisionAppleResult;

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
    /// Extracted camera EXIF metadata (`None` if unavailable or stripped).
    pub exif: Option<PhotoExif>,
    /// Extracted RAW camera dual-illuminant calibration matrices & as-shot WB (`None` for rasters/samples).
    pub camera_calibration: Option<CameraCalibration>,
}

impl LinearImage {
    pub fn byte_slice(&self) -> &[u8] {
        bytemuck::cast_slice(&self.rgba16f)
    }

    /// Returns `true` iff dimensions are non-zero and `rgba16f` holds at least `width * height * 4`
    /// elements without arithmetic overflow.
    pub(crate) fn has_valid_buffer(&self) -> bool {
        if self.width == 0 || self.height == 0 {
            return false;
        }
        (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|n| n.checked_mul(4))
            .is_some_and(|needed| self.rgba16f.len() >= needed)
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
    #[error("I/O error for '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Failed to decode RAW/native image '{path}': {message}")]
    Raw { path: PathBuf, message: String },
    #[error("Empty image dimensions ({0}x{1})")]
    EmptyDimensions(u32, u32),
    #[error("Invalid image buffer: {0}")]
    InvalidBuffer(String),
    #[error(
        "Refusing to overwrite original source photo '{0}'. Export to a different path or directory."
    )]
    SourceOverwrite(PathBuf),
    #[error("Invalid export path: {0}")]
    InvalidPath(String),
    #[error("Duplicate export destination '{0}'")]
    DuplicateDestination(PathBuf),
    #[error("Export destination already exists: '{0}' (enable overwrite to replace)")]
    DestinationExists(PathBuf),
}

/// Decodes a photo file (Camera RAW, ProRAW, HEIC, JPEG, PNG, TIFF, WebP) from disk
/// at full resolution into scene-referred linear `Rgba16Float`.
pub fn load_file(path: &Path) -> Result<LinearImage, DecodeError> {
    load_file_proxy(path, None)
}

/// Decodes an in-memory standard image (JPEG, PNG, TIFF, WebP) into scene-referred
/// linear `Rgba16Float` (used for bundled sample photography and unit tests).
pub fn load_from_memory(
    bytes: &[u8],
    display_name: &str,
    format_label: &str,
) -> Result<LinearImage, DecodeError> {
    let dyn_img = image::load_from_memory(bytes).map_err(|source| DecodeError::Image {
        path: PathBuf::from(display_name),
        source,
    })?;
    let rgba32f = dyn_img.to_rgba32f();
    let (width, height) = rgba32f.dimensions();
    if width == 0 || height == 0 {
        return Err(DecodeError::EmptyDimensions(width, height));
    }

    let mut rgba16f = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for px in rgba32f.pixels() {
        rgba16f.push(f16::from_f32(rune_editor_core::srgb_to_linear(px[0])));
        rgba16f.push(f16::from_f32(rune_editor_core::srgb_to_linear(px[1])));
        rgba16f.push(f16::from_f32(rune_editor_core::srgb_to_linear(px[2])));
        rgba16f.push(f16::from_f32(px[3].clamp(0.0, 1.0)));
    }
    let histogram_proxy = build_histogram_proxy(&rgba16f, width, height);
    let exif = extract_exif_from_bytes(bytes);

    Ok(LinearImage {
        width,
        height,
        rgba16f,
        histogram_proxy,
        display_name: display_name.to_owned(),
        source_path: None,
        format_label: format_label.to_owned(),
        exif,
        camera_calibration: None,
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
        exif: Some(PhotoExif {
            camera_model: Some("Leica SL3 (Demo)".to_owned()),
            lens_model: Some("APO-Summicron 35mm f/2 ASPH".to_owned()),
            focal_length_mm: Some(35.0),
            aperture_f: Some(2.8),
            shutter_display: Some("1/250s".to_owned()),
            iso: Some(100),
            date_taken: Some("2026:10:18 18:42:10".to_owned()),
        }),
        camera_calibration: None,
    }
}

pub(crate) fn build_histogram_proxy(rgba16f: &[f16], width: u32, height: u32) -> Vec<[f32; 3]> {
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
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use image::{ImageBuffer, Rgb};
    use rune_editor_core::DevelopRecipe;

    use super::*;

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "rune_decode_test_{label}_{}_{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write_top_red_bottom_blue_png(path: &Path, width: u32, height: u32) {
        let img = ImageBuffer::from_fn(width, height, |_x, y| {
            if y < height / 2 {
                Rgb([230u8, 30, 30])
            } else {
                Rgb([20u8, 40, 230])
            }
        });
        img.save(path).expect("save synthetic png");
    }

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
        assert!(
            max_val > 1.2,
            "demo scene should contain HDR >1.0 highlights, got {max_val}"
        );
    }

    #[test]
    fn extension_detection_covers_raw_and_raster_case_insensitively() {
        for ext in SUPPORTED_EXTENSIONS {
            let lower = PathBuf::from(format!("DSC0001.{ext}"));
            let upper = PathBuf::from(format!("DSC0001.{}", ext.to_ascii_uppercase()));
            assert!(is_supported_photo(&lower), "expected {ext} supported");
            assert!(is_supported_photo(&upper), "expected {ext} upper supported");
        }

        for raw_ext in [
            "dng", "CR2", "cr3", "NEF", "arw", "RAF", "orf", "rw2", "pef", "srw", "HEIC", "heif",
        ] {
            let p = PathBuf::from(format!("photo.{raw_ext}"));
            assert!(is_raw_extension(&p), "expected {raw_ext} to be RAW/HEIF");
            assert!(is_supported_photo(&p));
        }

        for raster_ext in ["jpg", "JPEG", "png", "tif", "TIFF", "webp"] {
            let p = PathBuf::from(format!("photo.{raster_ext}"));
            assert!(!is_raw_extension(&p), "expected {raster_ext} not to be RAW");
            assert!(is_supported_photo(&p));
        }

        for unsupported in ["notes.txt", "edit.rune-edit.json", "movie.mov", "no_ext"] {
            let p = PathBuf::from(unsupported);
            assert!(
                !is_supported_photo(&p),
                "unexpected support for {unsupported}"
            );
            assert!(!is_raw_extension(&p));
        }
    }

    #[test]
    fn scan_photo_folder_filters_sorts_and_skips_hidden_files() {
        let dir = unique_temp_dir("scan");
        fs::write(dir.join("zebra.NEF"), b"").unwrap();
        fs::write(dir.join("alpha.jpg"), b"").unwrap();
        fs::write(dir.join("Beta.DNG"), b"").unwrap();
        fs::write(dir.join("gamma.heic"), b"").unwrap();
        fs::write(dir.join("._Beta.DNG"), b"").unwrap();
        fs::write(dir.join(".DS_Store"), b"").unwrap();
        fs::write(dir.join("readme.txt"), b"").unwrap();
        fs::create_dir(dir.join("subdir.dng")).unwrap();

        let scanned = scan_photo_folder(&dir).expect("scan folder");
        let names: Vec<String> = scanned
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["alpha.jpg", "Beta.DNG", "gamma.heic", "zebra.NEF"]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn demo_and_file_thumbnails_preserve_aspect_and_orientation() {
        let demo = generate_demo_hdr_scene(320, 160);
        let demo_thumb = generate_demo_thumbnail(&demo, 128);
        assert_eq!(demo_thumb.width, 128);
        assert_eq!(demo_thumb.height, 64);
        assert_eq!(demo_thumb.rgba.len(), 128 * 64 * 4);
        assert!(demo_thumb.rgba.iter().any(|&b| b > 20));

        let dir = unique_temp_dir("thumb");
        let png_path = dir.join("orient.png");
        write_top_red_bottom_blue_png(&png_path, 160, 80);

        let thumb = load_thumbnail(&png_path, 64).expect("load thumbnail");
        assert_eq!(thumb.width, 64);
        assert_eq!(thumb.height, 32);
        assert_eq!(thumb.rgba.len(), 64 * 32 * 4);

        // Verify top-left pixel is red (not upside-down, not BGRA swapped) and bottom-left is blue.
        let top_px = &thumb.rgba[0..4];
        let bot_idx = ((thumb.height as usize - 1) * thumb.width as usize) * 4;
        let bot_px = &thumb.rgba[bot_idx..bot_idx + 4];
        assert!(
            top_px[0] > 180 && top_px[2] < 80,
            "expected top pixel to be red, got {top_px:?}"
        );
        assert!(
            bot_px[2] > 180 && bot_px[0] < 80,
            "expected bottom pixel to be blue, got {bot_px:?}"
        );

        // Also verify proxy scaling via load_file_proxy.
        let proxy = load_file_proxy(&png_path, Some(80)).expect("load proxy");
        assert_eq!(proxy.width, 80);
        assert_eq!(proxy.height, 40);

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_coreimage_linear_decode_preserves_orientation_and_scale() {
        let dir = unique_temp_dir("coreimage");
        let png_path = dir.join("ci_test.png");
        write_top_red_bottom_blue_png(&png_path, 120, 60);

        let decoded =
            raw::macos::decode_coreimage_linear(&png_path, Some(60), false).expect("CI decode");
        assert_eq!(decoded.width, 60);
        assert_eq!(decoded.height, 30);

        // Row 0 must be red, last row must be blue.
        let top_r = decoded.rgba16f[0].to_f32();
        let top_b = decoded.rgba16f[2].to_f32();
        let bot_base = ((decoded.height as usize - 1) * decoded.width as usize) * 4;
        let bot_r = decoded.rgba16f[bot_base].to_f32();
        let bot_b = decoded.rgba16f[bot_base + 2].to_f32();
        assert!(
            top_r > 0.6 && top_b < 0.1,
            "CoreImage top pixel should be linear red, got r={top_r}, b={top_b}"
        );
        assert!(
            bot_b > 0.6 && bot_r < 0.1,
            "CoreImage bottom pixel should be linear blue, got r={bot_r}, b={bot_b}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bilinear_bayer_demosaic_reconstructs_neutral_gray() {
        // 8x8 RGGB uniform sensor plane with gray value 0.25.
        let cfa = rawler::CFA::new("RGGB");
        let plane = vec![0.25f32; 8 * 8];
        let rgb = raw::bilinear_demosaic_plane(&plane, 8, 8, Some(&cfa), [1.0, 1.0, 1.0]);
        assert_eq!(rgb.len(), 64);
        for px in &rgb {
            assert!((px[0] - 0.25).abs() < 1e-5);
            assert!((px[1] - 0.25).abs() < 1e-5);
            assert!((px[2] - 0.25).abs() < 1e-5);
        }
    }

    #[test]
    fn export_roundtrips_jpeg_png8_png16_tiff16_and_reflects_exposure() {
        let dir = unique_temp_dir("export");

        // Uniform 18% middle-gray linear scene (64x48).
        let width = 64u32;
        let height = 48u32;
        let mut rgba16f = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            rgba16f.push(f16::from_f32(0.18));
            rgba16f.push(f16::from_f32(0.18));
            rgba16f.push(f16::from_f32(0.18));
            rgba16f.push(f16::ONE);
        }
        let image = LinearImage {
            width,
            height,
            histogram_proxy: build_histogram_proxy(&rgba16f, width, height),
            rgba16f,
            display_name: "Gray18.dng".to_owned(),
            source_path: None,
            format_label: "Test".to_owned(),
            exif: None,
            camera_calibration: None,
        };

        let neutral = DevelopRecipe::default();
        let bright = DevelopRecipe {
            exposure: 1.5,
            ..DevelopRecipe::default()
        };

        let cases = [
            (
                "neutral.jpg",
                "bright.jpg",
                ExportFormat::Jpeg { quality: 95 },
            ),
            ("neutral_8.png", "bright_8.png", ExportFormat::Png8),
            ("neutral_16.png", "bright_16.png", ExportFormat::Png16),
            ("neutral_16.tiff", "bright_16.tiff", ExportFormat::Tiff16),
        ];

        for (neutral_name, bright_name, fmt) in cases {
            let neutral_path = dir.join(neutral_name);
            let bright_path = dir.join(bright_name);

            export_image(&image, &neutral, &neutral_path, fmt).expect("export neutral");
            export_image(&image, &bright, &bright_path, fmt).expect("export bright");

            let dec_neutral = image::open(&neutral_path).expect("decode neutral export");
            let dec_bright = image::open(&bright_path).expect("decode bright export");

            assert_eq!(dec_neutral.width(), width);
            assert_eq!(dec_neutral.height(), height);
            assert_eq!(dec_bright.width(), width);
            assert_eq!(dec_bright.height(), height);

            // Verify bit depth for 16-bit exports.
            if matches!(fmt, ExportFormat::Png16 | ExportFormat::Tiff16) {
                assert!(
                    matches!(dec_neutral, image::DynamicImage::ImageRgb16(_)),
                    "expected 16-bit RGB output for {neutral_name}"
                );
            }

            let mean_luma = |img: &image::DynamicImage| -> f32 {
                let rgb = img.to_rgb32f();
                let sum: f32 = rgb.pixels().map(|p| (p[0] + p[1] + p[2]) / 3.0).sum();
                sum / (rgb.width() * rgb.height()) as f32
            };

            let l_neutral = mean_luma(&dec_neutral);
            let l_bright = mean_luma(&dec_bright);
            assert!(
                l_bright > l_neutral + 0.15,
                "exposure +1.5 EV should brighten {bright_name} (neutral={l_neutral:.3}, bright={l_bright:.3})"
            );
        }

        // Verify vignette darkens corners relative to center.
        let vig_path = dir.join("vignette.png");
        let vig_recipe = DevelopRecipe {
            vignette: -0.8,
            grain: 0.0,
            ..DevelopRecipe::default()
        };
        export_image(&image, &vig_recipe, &vig_path, ExportFormat::Png8).expect("export vignette");
        let vig_img = image::open(&vig_path).expect("decode vignette").to_rgb8();
        let center = vig_img.get_pixel(width / 2, height / 2)[0];
        let corner = vig_img.get_pixel(0, 0)[0];
        assert!(
            corner + 15 < center,
            "negative vignette should darken corner ({corner}) vs center ({center})"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_applies_vignette_and_grain_in_scene_linear_before_filmic_tonemap() {
        let dir = unique_temp_dir("hdr_vignette_grain");
        let width = 32u32;
        let height = 32u32;

        // Uniform 3.0 linear HDR highlight well above the 0.75 filmic shoulder.
        let mut rgba16f = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            rgba16f.push(f16::from_f32(3.0));
            rgba16f.push(f16::from_f32(3.0));
            rgba16f.push(f16::from_f32(3.0));
            rgba16f.push(f16::ONE);
        }
        let hdr = LinearImage {
            width,
            height,
            histogram_proxy: build_histogram_proxy(&rgba16f, width, height),
            rgba16f,
            display_name: "HDR_Highlight.dng".to_owned(),
            source_path: None,
            format_label: "Test".to_owned(),
            exif: None,
            camera_calibration: None,
        };

        let recipe = DevelopRecipe {
            vignette: -1.0,
            grain: 0.25,
            ..DevelopRecipe::default()
        };
        let out_path = dir.join("hdr_vig_grain.png");
        export_image(&hdr, &recipe, &out_path, ExportFormat::Png16).expect("export hdr png16");

        let decoded = image::open(&out_path).expect("decode hdr png16").to_rgb16();
        let corner_srgb = decoded.get_pixel(0, 0)[0] as f32 / 65535.0;
        let center_srgb = decoded.get_pixel(width / 2, height / 2)[0] as f32 / 65535.0;

        // Pre-tonemap: 3.0 * vig_gain (~0.45) ≈ 1.35 linear, which still sits above the 0.75
        // filmic shoulder (~0.92 linear -> ~0.96 sRGB). Post-tonemap would crush it to ~0.69 sRGB.
        assert!(
            corner_srgb > 0.90 && corner_srgb < center_srgb,
            "expected HDR pre-tonemap vignette corner in (0.90, center={center_srgb:.4}), got {corner_srgb:.4}"
        );

        // Verify grain introduces subtle pixel-to-pixel variation in midtones too.
        let mut mid_rgba16f = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            mid_rgba16f.push(f16::from_f32(0.18));
            mid_rgba16f.push(f16::from_f32(0.18));
            mid_rgba16f.push(f16::from_f32(0.18));
            mid_rgba16f.push(f16::ONE);
        }
        let mid = LinearImage {
            width,
            height,
            histogram_proxy: build_histogram_proxy(&mid_rgba16f, width, height),
            rgba16f: mid_rgba16f,
            display_name: "Mid18.dng".to_owned(),
            source_path: None,
            format_label: "Test".to_owned(),
            exif: None,
            camera_calibration: None,
        };
        let grain_path = dir.join("mid_grain.png");
        export_image(&mid, &recipe, &grain_path, ExportFormat::Png16).expect("export grain png16");
        let grain_img = image::open(&grain_path)
            .expect("decode grain png16")
            .to_rgb16();
        let p0 = grain_img.get_pixel(14, 14)[0];
        let p1 = grain_img.get_pixel(15, 14)[0];
        let p2 = grain_img.get_pixel(16, 14)[0];
        assert!(
            p0 != p1 || p1 != p2,
            "expected film grain to vary adjacent midtone pixels, got {p0}, {p1}, {p2}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn exif_exposure_summary_and_shutter_formatting() {
        assert_eq!(format_shutter_speed(1.0 / 250.0).as_deref(), Some("1/250s"));
        assert_eq!(format_shutter_speed(0.5).as_deref(), Some("0.5s"));
        assert_eq!(format_shutter_speed(2.0).as_deref(), Some("2s"));
        assert_eq!(format_shutter_speed(1.5).as_deref(), Some("1.5s"));
        assert_eq!(format_shutter_speed(0.0), None);
        assert_eq!(format_shutter_speed(-0.1), None);
        assert_eq!(format_shutter_speed(f64::NAN), None);

        let full = PhotoExif {
            camera_model: Some("Sony ILCE-7RM5".to_owned()),
            lens_model: Some("FE 50mm F1.4 GM".to_owned()),
            focal_length_mm: Some(50.0),
            aperture_f: Some(2.0),
            shutter_display: Some("1/250s".to_owned()),
            iso: Some(400),
            date_taken: Some("2026:05:12 14:30:00".to_owned()),
        };
        assert!(!full.is_empty());
        assert_eq!(full.exposure_summary(), "50mm • f/2.0 • 1/250s • ISO 400");

        let partial = PhotoExif {
            focal_length_mm: Some(24.5),
            iso: Some(800),
            ..PhotoExif::default()
        };
        assert_eq!(partial.exposure_summary(), "24.5mm • ISO 800");

        let empty = PhotoExif::default();
        assert!(empty.is_empty());
        assert_eq!(empty.exposure_summary(), "");
    }

    /// Builds a valid minimal JPEG with an embedded APP1 Exif TIFF block containing
    /// TIFF Model + Exif LensModel, FocalLength, FNumber, ExposureTime, ISOSpeedRatings, DateTimeOriginal.
    #[cfg(target_os = "macos")]
    fn build_synthetic_exif_jpeg() -> Vec<u8> {
        use image::codecs::jpeg::JpegEncoder;

        let mut base_jpg = Vec::new();
        let px = vec![128u8; 4 * 4 * 3];
        JpegEncoder::new_with_quality(&mut base_jpg, 90)
            .encode(&px, 4, 4, image::ExtendedColorType::Rgb8)
            .expect("encode tiny jpeg");
        assert_eq!(&base_jpg[0..2], &[0xFF, 0xD8]);

        // Construct big-endian ("MM") TIFF payload.
        let mut tiff = Vec::<u8>::new();
        tiff.extend_from_slice(b"MM");
        tiff.extend_from_slice(&42u16.to_be_bytes());
        tiff.extend_from_slice(&8u32.to_be_bytes()); // IFD0 offset = 8

        let model_bytes = b"Leica Q3\0";
        let lens_bytes = b"Summilux 28mm f/1.7\0";
        let date_bytes = b"2026:10:18 18:42:10\0";

        // IFD0: 2 entries (Model 0x0110, ExifIFDPointer 0x8769)
        // IFD0 starts at 8: 2 (count) + 2*12 (entries) + 4 (next=0) = 36 -> offset 36
        let ifd0_count: u16 = 2;
        let exif_ifd_offset: u32 = 8 + 2 + (ifd0_count as u32) * 12 + 4; // 36
        // Exif IFD: 6 entries (ExposureTime, FNumber, ISOSpeedRatings, DateTimeOriginal, FocalLength, LensModel)
        let exif_count: u16 = 6;
        let data_start: u32 = exif_ifd_offset + 2 + (exif_count as u32) * 12 + 4; // 36 + 78 = 114

        // Reserve space for data payloads placed sequentially starting at data_start.
        let model_off = data_start;
        let exp_off = model_off + model_bytes.len() as u32;
        let fnum_off = exp_off + 8;
        let date_off = fnum_off + 8;
        let fl_off = date_off + date_bytes.len() as u32;
        let lens_off = fl_off + 8;

        let push_entry = |buf: &mut Vec<u8>, tag: u16, typ: u16, count: u32, val: u32| {
            buf.extend_from_slice(&tag.to_be_bytes());
            buf.extend_from_slice(&typ.to_be_bytes());
            buf.extend_from_slice(&count.to_be_bytes());
            buf.extend_from_slice(&val.to_be_bytes());
        };

        // Write IFD0
        tiff.extend_from_slice(&ifd0_count.to_be_bytes());
        push_entry(&mut tiff, 0x0110, 2, model_bytes.len() as u32, model_off);
        push_entry(&mut tiff, 0x8769, 4, 1, exif_ifd_offset);
        tiff.extend_from_slice(&0u32.to_be_bytes());
        assert_eq!(tiff.len() as u32, exif_ifd_offset);

        // Write Exif IFD (sorted by tag ID per TIFF spec)
        tiff.extend_from_slice(&exif_count.to_be_bytes());
        push_entry(&mut tiff, 0x829A, 5, 1, exp_off); // ExposureTime: 1/250s
        push_entry(&mut tiff, 0x829D, 5, 1, fnum_off); // FNumber: 20/10 = f/2.0
        push_entry(&mut tiff, 0x8827, 3, 1, 400u32 << 16); // ISOSpeedRatings: SHORT(400) in upper 16 bits (BE)
        push_entry(&mut tiff, 0x9003, 2, date_bytes.len() as u32, date_off); // DateTimeOriginal
        push_entry(&mut tiff, 0x920A, 5, 1, fl_off); // FocalLength: 28/1 = 28mm
        push_entry(&mut tiff, 0xA434, 2, lens_bytes.len() as u32, lens_off); // LensModel
        tiff.extend_from_slice(&0u32.to_be_bytes());
        assert_eq!(tiff.len() as u32, data_start);

        // Data area
        tiff.extend_from_slice(model_bytes);
        tiff.extend_from_slice(&1u32.to_be_bytes());
        tiff.extend_from_slice(&250u32.to_be_bytes());
        tiff.extend_from_slice(&20u32.to_be_bytes());
        tiff.extend_from_slice(&10u32.to_be_bytes());
        tiff.extend_from_slice(date_bytes);
        tiff.extend_from_slice(&28u32.to_be_bytes());
        tiff.extend_from_slice(&1u32.to_be_bytes());
        tiff.extend_from_slice(lens_bytes);

        // Splice APP1 ("Exif\0\0" + tiff) right after SOI.
        let mut app1_payload = b"Exif\0\0".to_vec();
        app1_payload.extend_from_slice(&tiff);
        let seg_len = (app1_payload.len() + 2) as u16;

        let mut out = Vec::with_capacity(base_jpg.len() + app1_payload.len() + 4);
        out.extend_from_slice(&base_jpg[0..2]);
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&seg_len.to_be_bytes());
        out.extend_from_slice(&app1_payload);
        out.extend_from_slice(&base_jpg[2..]);
        out
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_exif_extraction_reads_memory_and_disk_jpeg_metadata() {
        let jpeg_bytes = build_synthetic_exif_jpeg();

        let from_mem =
            load_from_memory(&jpeg_bytes, "LeicaTest.jpg", "JPEG").expect("load memory jpeg");
        let exif = from_mem.exif.expect("expected EXIF from memory JPEG");
        assert_eq!(exif.camera_model.as_deref(), Some("Leica Q3"));
        assert_eq!(exif.lens_model.as_deref(), Some("Summilux 28mm f/1.7"));
        assert_eq!(exif.focal_length_mm, Some(28.0));
        assert_eq!(exif.aperture_f, Some(2.0));
        assert_eq!(exif.shutter_display.as_deref(), Some("1/250s"));
        assert_eq!(exif.iso, Some(400));
        assert_eq!(exif.date_taken.as_deref(), Some("2026:10:18 18:42:10"));
        assert_eq!(exif.exposure_summary(), "28mm • f/2.0 • 1/250s • ISO 400");

        let dir = unique_temp_dir("exif_disk");
        let jpg_path = dir.join("leica.jpg");
        fs::write(&jpg_path, &jpeg_bytes).expect("write exif jpeg");
        let from_disk = load_file_proxy(&jpg_path, None).expect("load disk jpeg");
        assert_eq!(from_disk.exif.as_ref(), Some(&exif));

        // Plain PNG without EXIF should gracefully yield None.
        let png_path = dir.join("plain.png");
        write_top_red_bottom_blue_png(&png_path, 16, 16);
        let plain = load_file_proxy(&png_path, None).expect("load plain png");
        assert_eq!(plain.exif, None);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_webp_max_edge_and_output_sharpen_work_cleanly() {
        let dir = unique_temp_dir("export_pro");
        let width = 120u32;
        let height = 60u32;

        // Sharp vertical step-edge scene (left dark gray, right bright gray) to test sharpening + resize.
        let mut rgba16f = Vec::with_capacity((width * height * 4) as usize);
        for _y in 0..height {
            for x in 0..width {
                let v = if x < width / 2 { 0.08 } else { 0.36 };
                rgba16f.push(f16::from_f32(v));
                rgba16f.push(f16::from_f32(v));
                rgba16f.push(f16::from_f32(v));
                rgba16f.push(f16::ONE);
            }
        }
        let img = LinearImage {
            width,
            height,
            histogram_proxy: build_histogram_proxy(&rgba16f, width, height),
            rgba16f,
            display_name: "StepEdge.dng".to_owned(),
            source_path: None,
            format_label: "Test".to_owned(),
            exif: None,
            camera_calibration: None,
        };
        let recipe = DevelopRecipe::default();

        // 1. WebP export + max_edge downscale (120x60 -> 60x30)
        let webp_path = dir.join("scaled.webp");
        export_image_with_options(
            &img,
            &recipe,
            &webp_path,
            &ExportOptions {
                format: ExportFormat::WebP,
                max_edge: Some(60),
                respect_crop: true,
                output_sharpen: 0.0,
                ..ExportOptions::default()
            },
        )
        .expect("export webp");
        let dec_webp = image::open(&webp_path).expect("decode webp");
        assert_eq!(dec_webp.width(), 60);
        assert_eq!(dec_webp.height(), 30);

        // 2. Output sharpening increases local contrast right across the step edge.
        let unsharp_off = dir.join("unsharp_off.png");
        let unsharp_on = dir.join("unsharp_on.png");
        export_image_with_options(
            &img,
            &recipe,
            &unsharp_off,
            &ExportOptions {
                format: ExportFormat::Png16,
                max_edge: None,
                respect_crop: true,
                output_sharpen: 0.0,
                ..ExportOptions::default()
            },
        )
        .expect("export unsharp off");
        export_image_with_options(
            &img,
            &recipe,
            &unsharp_on,
            &ExportOptions {
                format: ExportFormat::Png16,
                max_edge: None,
                respect_crop: true,
                output_sharpen: 0.8,
                ..ExportOptions::default()
            },
        )
        .expect("export unsharp on");

        let off = image::open(&unsharp_off).expect("decode off").to_rgb16();
        let on = image::open(&unsharp_on).expect("decode on").to_rgb16();
        let edge_diff_off = off.get_pixel(width / 2, height / 2)[0] as i32
            - off.get_pixel(width / 2 - 1, height / 2)[0] as i32;
        let edge_diff_on = on.get_pixel(width / 2, height / 2)[0] as i32
            - on.get_pixel(width / 2 - 1, height / 2)[0] as i32;
        assert!(
            edge_diff_on > edge_diff_off + 1000,
            "expected output_sharpen to boost edge contrast (off={edge_diff_off}, on={edge_diff_on})"
        );
        // Flat interior far from edge should be unchanged.
        assert_eq!(
            off.get_pixel(5, height / 2)[0],
            on.get_pixel(5, height / 2)[0]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_respects_geometry_crop_rotate_flip_and_straighten_parity() {
        use rune_editor_core::GeometryParams;

        let dir = unique_temp_dir("export_geo");
        // 80x40 4-quadrant test chart:
        // Top-Left = Red, Top-Right = Green, Bottom-Left = Blue, Bottom-Right = White
        let width = 80u32;
        let height = 40u32;
        let mut rgba16f = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let rgb = match (x < width / 2, y < height / 2) {
                    (true, true) => [0.5, 0.01, 0.01],  // TL: Red
                    (false, true) => [0.01, 0.5, 0.01], // TR: Green
                    (true, false) => [0.01, 0.01, 0.5], // BL: Blue
                    (false, false) => [0.5, 0.5, 0.5],  // BR: White
                };
                rgba16f.push(f16::from_f32(rgb[0]));
                rgba16f.push(f16::from_f32(rgb[1]));
                rgba16f.push(f16::from_f32(rgb[2]));
                rgba16f.push(f16::ONE);
            }
        }
        let img = LinearImage {
            width,
            height,
            histogram_proxy: build_histogram_proxy(&rgba16f, width, height),
            rgba16f,
            display_name: "Quadrants.dng".to_owned(),
            source_path: None,
            format_label: "Test".to_owned(),
            exif: None,
            camera_calibration: None,
        };

        // 1. Crop to Top-Right quadrant (Green): [0.5, 0.0, 0.5, 0.5] -> 40x20
        let crop_recipe = DevelopRecipe {
            geometry: GeometryParams {
                crop: [0.5, 0.0, 0.5, 0.5],
                ..GeometryParams::default()
            },
            ..DevelopRecipe::default()
        };
        let crop_path = dir.join("crop_tr.png");
        export_image(&img, &crop_recipe, &crop_path, ExportFormat::Png8).expect("export crop");
        let dec_crop = image::open(&crop_path).expect("open crop").to_rgb8();
        assert_eq!(dec_crop.dimensions(), (40, 20));
        let px = dec_crop.get_pixel(20, 10);
        assert!(
            px[1] > 150 && px[0] < 60 && px[2] < 60,
            "expected green TR crop, got {px:?}"
        );

        // 2. respect_crop = false bypasses crop and keeps full 80x40 dimensions.
        let uncropped_path = dir.join("uncropped.png");
        export_image_with_options(
            &img,
            &crop_recipe,
            &uncropped_path,
            &ExportOptions {
                format: ExportFormat::Png8,
                max_edge: None,
                respect_crop: false,
                output_sharpen: 0.0,
                ..ExportOptions::default()
            },
        )
        .expect("export uncropped");
        let dec_uncropped = image::open(&uncropped_path).expect("open uncropped");
        assert_eq!((dec_uncropped.width(), dec_uncropped.height()), (80, 40));

        // 3. Rotate 90° CW: 80x40 -> 40x80; old BL (Blue) becomes new TL, old TL (Red) becomes new TR.
        let rot_recipe = DevelopRecipe {
            geometry: GeometryParams {
                rotate_90: 1,
                ..GeometryParams::default()
            },
            ..DevelopRecipe::default()
        };
        let rot_path = dir.join("rot90.png");
        export_image(&img, &rot_recipe, &rot_path, ExportFormat::Png8).expect("export rot90");
        let dec_rot = image::open(&rot_path).expect("open rot90").to_rgb8();
        assert_eq!(dec_rot.dimensions(), (40, 80));
        let tl = dec_rot.get_pixel(5, 5);
        let tr = dec_rot.get_pixel(34, 5);
        assert!(
            tl[2] > 150 && tl[0] < 60,
            "rot90 TL should be Blue, got {tl:?}"
        );
        assert!(
            tr[0] > 150 && tr[2] < 60,
            "rot90 TR should be Red, got {tr:?}"
        );

        // 4. Flip H + Flip V: TL (Red) and BR (White) swap; TR (Green) and BL (Blue) swap.
        let flip_recipe = DevelopRecipe {
            geometry: GeometryParams {
                flip_h: true,
                flip_v: true,
                ..GeometryParams::default()
            },
            ..DevelopRecipe::default()
        };
        let flip_path = dir.join("flip_hv.png");
        export_image(&img, &flip_recipe, &flip_path, ExportFormat::Png8).expect("export flip");
        let dec_flip = image::open(&flip_path).expect("open flip").to_rgb8();
        let tl = dec_flip.get_pixel(10, 5);
        let br = dec_flip.get_pixel(70, 35);
        assert!(
            tl[0] > 150 && tl[1] > 150 && tl[2] > 150,
            "flip_hv TL should be White, got {tl:?}"
        );
        assert!(
            br[0] > 150 && br[1] < 60 && br[2] < 60,
            "flip_hv BR should be Red, got {br:?}"
        );

        // 5. Straighten + degenerate crop safety (NaN/negative/overflow crop + straighten angle).
        let straight_recipe = DevelopRecipe {
            geometry: GeometryParams {
                crop: [f32::NAN, -1.0, 0.0, 2.5],
                straighten_deg: 15.0,
                ..GeometryParams::default()
            },
            ..DevelopRecipe::default()
        };
        let straight_path = dir.join("straight.png");
        export_image(&img, &straight_recipe, &straight_path, ExportFormat::Png8)
            .expect("export degenerate crop + straighten");
        let dec_straight = image::open(&straight_path).expect("open straight");
        assert!(dec_straight.width() >= 1 && dec_straight.height() >= 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn spatial_detail_export_applies_nr_clarity_structure_and_sharpen() {
        let dir = unique_temp_dir("spatial_detail");
        let width = 48u32;
        let height = 48u32;

        // Synthetic linear scene:
        // - Left half: dark-midtone (0.14) with checkerboard high-frequency chroma/luma noise.
        // - Right half: brighter midtone (0.38) with fine alternating stripes.
        let mut rgba16f = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let rgb = if x < width / 2 {
                    let odd = (x + y) % 2 == 0;
                    if odd {
                        [0.18, 0.12, 0.12]
                    } else {
                        [0.10, 0.16, 0.16]
                    }
                } else {
                    let stripe = if x % 2 == 0 { 0.42 } else { 0.34 };
                    [stripe, stripe, stripe]
                };
                rgba16f.push(f16::from_f32(rgb[0]));
                rgba16f.push(f16::from_f32(rgb[1]));
                rgba16f.push(f16::from_f32(rgb[2]));
                rgba16f.push(f16::ONE);
            }
        }
        let img = LinearImage {
            width,
            height,
            histogram_proxy: build_histogram_proxy(&rgba16f, width, height),
            rgba16f,
            display_name: "SpatialDetail.dng".to_owned(),
            source_path: None,
            format_label: "Test".to_owned(),
            exif: None,
            camera_calibration: None,
        };

        let export_rgb16 = |name: &str, recipe: &DevelopRecipe| {
            let p = dir.join(name);
            export_image(&img, recipe, &p, ExportFormat::Png16).expect("export png16");
            image::open(&p).expect("open png16").to_rgb16()
        };

        let base = export_rgb16("base.png", &DevelopRecipe::default());

        // 1. Noise Reduction reduces chroma/luma alternating ripple on the noisy left half.
        let nr_recipe = DevelopRecipe {
            noise_reduction: 0.85,
            ..DevelopRecipe::default()
        };
        let nr_img = export_rgb16("nr.png", &nr_recipe);
        let chroma_diff = |buf: &image::ImageBuffer<Rgb<u16>, Vec<u16>>| -> i64 {
            let p = buf.get_pixel(10, 24);
            (p[0] as i64 - p[1] as i64).abs()
        };
        assert!(
            chroma_diff(&nr_img) < chroma_diff(&base) / 2,
            "NR should smooth chroma noise (base={}, nr={})",
            chroma_diff(&base),
            chroma_diff(&nr_img)
        );

        // 2. Clarity boosts broad midtone step contrast across the center seam.
        let clarity_recipe = DevelopRecipe {
            clarity: 0.8,
            ..DevelopRecipe::default()
        };
        let clarity_img = export_rgb16("clarity.png", &clarity_recipe);
        let seam_contrast = |buf: &image::ImageBuffer<Rgb<u16>, Vec<u16>>| -> i64 {
            buf.get_pixel(width / 2 + 2, 24)[0] as i64 - buf.get_pixel(width / 2 - 2, 24)[0] as i64
        };
        assert!(
            seam_contrast(&clarity_img) > seam_contrast(&base) + 500,
            "Clarity should boost broad midtone contrast (base={}, clarity={})",
            seam_contrast(&base),
            seam_contrast(&clarity_img)
        );

        // 3. Structure boosts fine texture contrast on the right-half stripes.
        let struct_recipe = DevelopRecipe {
            structure: 0.8,
            ..DevelopRecipe::default()
        };
        let struct_img = export_rgb16("structure.png", &struct_recipe);
        let stripe_diff = |buf: &image::ImageBuffer<Rgb<u16>, Vec<u16>>| -> i64 {
            (buf.get_pixel(36, 24)[0] as i64 - buf.get_pixel(37, 24)[0] as i64).abs()
        };
        assert!(
            stripe_diff(&struct_img) > stripe_diff(&base) + 400,
            "Structure should boost fine stripe micro-contrast (base={}, structure={})",
            stripe_diff(&base),
            stripe_diff(&struct_img)
        );

        // 4. Sharpen boosts tight 1px edge contrast and is deterministic across runs.
        let sharp_recipe = DevelopRecipe {
            sharpen: 0.85,
            ..DevelopRecipe::default()
        };
        let sharp_a = export_rgb16("sharp_a.png", &sharp_recipe);
        let sharp_b = export_rgb16("sharp_b.png", &sharp_recipe);
        assert_eq!(sharp_a.as_raw(), sharp_b.as_raw());
        assert!(
            stripe_diff(&sharp_a) > stripe_diff(&base) + 500,
            "Sharpen should boost 1px edge contrast (base={}, sharpen={})",
            stripe_diff(&base),
            stripe_diff(&sharp_a)
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_image_to_jpeg_vec_applies_geometry_detail_and_max_edge() {
        use rune_editor_core::GeometryParams;

        let img = generate_demo_hdr_scene(160, 120);
        let recipe = DevelopRecipe {
            exposure: 0.25,
            clarity: 0.35,
            structure: 0.25,
            sharpen: 0.30,
            noise_reduction: 0.20,
            vignette: -0.35,
            grain: 0.15,
            geometry: GeometryParams {
                crop: [0.0, 0.0, 0.5, 1.0],
                ..GeometryParams::default()
            },
            ..DevelopRecipe::default()
        };

        let bytes = export_image_to_jpeg_vec(&img, &recipe, 60, 88).expect("jpeg vec");
        assert!(bytes.len() > 200);
        assert_eq!(&bytes[0..2], &[0xFF, 0xD8], "expected JPEG SOI header");

        let decoded = image::load_from_memory(&bytes).expect("decode jpeg vec");
        // Cropped 80x120 scaled to max_edge=60 -> 40x60.
        assert_eq!((decoded.width(), decoded.height()), (40, 60));

        // Deterministic output for identical inputs.
        let bytes2 = export_image_to_jpeg_vec(&img, &recipe, 60, 88).expect("jpeg vec 2");
        assert_eq!(bytes, bytes2);
    }

    #[test]
    fn probe_image_dimensions_reads_raster_and_respects_exif_orientation() {
        let dir = unique_temp_dir("probe_dims");
        let png_path = dir.join("wide.png");
        write_top_red_bottom_blue_png(&png_path, 160, 80);

        let dims = probe_image_dimensions(&png_path).expect("probe png");
        assert_eq!(dims, [160, 80]);

        let missing = probe_image_dimensions(&dir.join("missing.png"));
        assert!(matches!(missing, Err(DecodeError::Io { .. })));

        #[cfg(target_os = "macos")]
        {
            use image::codecs::jpeg::JpegEncoder;

            // Build a 16x8 JPEG carrying EXIF Orientation = 6 (Rotate 90° CW -> upright 8x16).
            let mut base_jpg = Vec::new();
            let px = vec![128u8; 16 * 8 * 3];
            JpegEncoder::new_with_quality(&mut base_jpg, 90)
                .encode(&px, 16, 8, image::ExtendedColorType::Rgb8)
                .expect("encode 16x8 jpeg");

            // Minimal big-endian TIFF with 1 IFD0 entry: Orientation (0x0112), SHORT(3), count=1, val=6.
            let mut tiff = Vec::new();
            tiff.extend_from_slice(b"MM");
            tiff.extend_from_slice(&42u16.to_be_bytes());
            tiff.extend_from_slice(&8u32.to_be_bytes());
            tiff.extend_from_slice(&1u16.to_be_bytes());
            tiff.extend_from_slice(&0x0112u16.to_be_bytes());
            tiff.extend_from_slice(&3u16.to_be_bytes());
            tiff.extend_from_slice(&1u32.to_be_bytes());
            tiff.extend_from_slice(&(6u32 << 16).to_be_bytes());
            tiff.extend_from_slice(&0u32.to_be_bytes());

            let mut app1 = b"Exif\0\0".to_vec();
            app1.extend_from_slice(&tiff);
            let seg_len = (app1.len() + 2) as u16;

            let mut oriented_jpg = Vec::with_capacity(base_jpg.len() + app1.len() + 4);
            oriented_jpg.extend_from_slice(&base_jpg[0..2]);
            oriented_jpg.extend_from_slice(&[0xFF, 0xE1]);
            oriented_jpg.extend_from_slice(&seg_len.to_be_bytes());
            oriented_jpg.extend_from_slice(&app1);
            oriented_jpg.extend_from_slice(&base_jpg[2..]);

            let oriented_path = dir.join("oriented_rot90.jpg");
            fs::write(&oriented_path, &oriented_jpg).expect("write oriented jpeg");
            let probed = probe_image_dimensions(&oriented_path).expect("probe oriented jpeg");
            assert_eq!(
                probed,
                [8, 16],
                "EXIF Orientation=6 must swap 16x8 to upright 8x16"
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn guard_not_source_blocks_same_and_symlinked_paths() {
        let dir = unique_temp_dir("guard_src");
        let real_sub = dir.join("real");
        fs::create_dir_all(&real_sub).expect("create real subdir");
        let src = real_sub.join("DSC0001.ARW");
        fs::write(&src, b"raw").expect("write src");

        // Same exact path is blocked.
        assert!(matches!(
            guard_not_source(&src, &src),
            Err(DecodeError::SourceOverwrite(_))
        ));

        // Relative `..` traversing through a non-existent subdir back to src is blocked.
        let tricky = real_sub
            .join("nonexistent_sub")
            .join("..")
            .join("DSC0001.ARW");
        assert!(matches!(
            guard_not_source(&src, &tricky),
            Err(DecodeError::SourceOverwrite(_))
        ));

        // Different file in same dir is allowed.
        let other = real_sub.join("DSC0001.jpg");
        assert!(guard_not_source(&src, &other).is_ok());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link_dir = dir.join("alias_dir");
            symlink(&real_sub, &link_dir).expect("create dir symlink");

            // Symlinked parent pointing to existing src is blocked.
            assert!(matches!(
                guard_not_source(&src, &link_dir.join("DSC0001.ARW")),
                Err(DecodeError::SourceOverwrite(_))
            ));

            // Even when both paths do not exist yet, canonicalizing parent symlinks catches collision.
            let future_src = real_sub.join("future.dng");
            let future_dest = link_dir.join("future.dng");
            assert!(matches!(
                guard_not_source(&future_src, &future_dest),
                Err(DecodeError::SourceOverwrite(_))
            ));
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_photo_folder_skips_symlink_cycles_and_caps_depth() {
        let dir = unique_temp_dir("symlink_cycle");
        let sub = dir.join("shoot");
        fs::create_dir_all(&sub).expect("create shoot dir");
        fs::write(sub.join("frame01.jpg"), b"").expect("write frame");

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            // Create a direct parent cycle (`shoot/loop -> dir`) and self-loop (`shoot/self -> shoot`).
            symlink(&dir, sub.join("loop")).expect("create parent symlink loop");
            symlink(&sub, sub.join("self_loop")).expect("create self symlink loop");
        }

        let scanned = scan_photo_folder_with_options(
            &dir,
            &ScanOptions {
                recursive: true,
                extensions: Vec::new(),
            },
        )
        .expect("recursive scan should survive symlink cycles");
        assert_eq!(scanned.len(), 1);
        assert_eq!(
            scanned[0].file_name().and_then(|s| s.to_str()),
            Some("frame01.jpg")
        );

        // Verify MAX_SCAN_DEPTH cap prevents unbounded deep recursion.
        let deep_root = unique_temp_dir("depth_cap");
        let mut cursor = deep_root.clone();
        for i in 0..=(MAX_SCAN_DEPTH + 2) {
            cursor = cursor.join(format!("d{i}"));
            fs::create_dir_all(&cursor).expect("mkdir deep");
        }
        fs::write(cursor.join("too_deep.jpg"), b"").expect("write too_deep");
        fs::write(deep_root.join("d0").join("shallow.jpg"), b"").expect("write shallow");

        let deep_scanned = scan_photo_folder_with_options(
            &deep_root,
            &ScanOptions {
                recursive: true,
                extensions: Vec::new(),
            },
        )
        .expect("deep scan");
        let names: Vec<String> = deep_scanned
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["shallow.jpg"]);

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&deep_root);
    }

    #[test]
    fn phase1_export_clarity_methods_nr_color_sharpen_controls_grain_types_and_film_curves() {
        use rune_editor_core::{ClarityMethod, FilmCurve, GrainType};

        let img = generate_demo_hdr_scene(64, 48);

        // 1. FilmCurve modes produce distinct exports in shadows/highlights.
        let (_, _, std_srgb) = export::render_developed_srgb_f32(
            &img,
            &DevelopRecipe {
                film_curve: FilmCurve::FilmStandard,
                ..DevelopRecipe::default()
            },
            &ExportOptions::default(),
        )
        .expect("standard curve");
        let (_, _, ext_srgb) = export::render_developed_srgb_f32(
            &img,
            &DevelopRecipe {
                film_curve: FilmCurve::FilmExtraShadow,
                ..DevelopRecipe::default()
            },
            &ExportOptions::default(),
        )
        .expect("extra shadow curve");
        let mean = |buf: &[[f32; 3]]| buf.iter().map(|p| p[0] + p[1] + p[2]).sum::<f32>();
        assert!(mean(&ext_srgb) > mean(&std_srgb));

        // 2. nr_color smooths chroma noise without softening pure luminance stripes.
        let mut chroma_rgba = Vec::with_capacity(32 * 32 * 4);
        for y in 0..32u32 {
            for x in 0..32u32 {
                let odd = (x + y) % 2 == 0;
                let rgb = if odd {
                    [0.24, 0.16, 0.16]
                } else {
                    [0.12, 0.20, 0.20]
                };
                chroma_rgba.push(f16::from_f32(rgb[0]));
                chroma_rgba.push(f16::from_f32(rgb[1]));
                chroma_rgba.push(f16::from_f32(rgb[2]));
                chroma_rgba.push(f16::ONE);
            }
        }
        let chroma_img = LinearImage {
            width: 32,
            height: 32,
            histogram_proxy: build_histogram_proxy(&chroma_rgba, 32, 32),
            rgba16f: chroma_rgba,
            display_name: "ChromaNoise.dng".to_owned(),
            source_path: None,
            format_label: "Test".to_owned(),
            exif: None,
            camera_calibration: None,
        };
        let (_, _, c_base) = export::render_developed_srgb_f32(
            &chroma_img,
            &DevelopRecipe::default(),
            &ExportOptions::default(),
        )
        .expect("base chroma");
        let (_, _, c_nr) = export::render_developed_srgb_f32(
            &chroma_img,
            &DevelopRecipe {
                nr_color: 0.9,
                ..DevelopRecipe::default()
            },
            &ExportOptions::default(),
        )
        .expect("nr_color");
        let idx = 16 * 32 + 16;
        assert!(
            (c_nr[idx][0] - c_nr[idx][1]).abs() < (c_base[idx][0] - c_base[idx][1]).abs() * 0.5,
            "nr_color should smooth chroma noise"
        );

        // 3. ClarityMethod Punch vs Neutral + Sharpen halo suppression + GrainType Harsh vs Soft.
        let (_, _, punch) = export::render_developed_srgb_f32(
            &img,
            &DevelopRecipe {
                clarity: 0.7,
                clarity_method: ClarityMethod::Punch,
                ..DevelopRecipe::default()
            },
            &ExportOptions::default(),
        )
        .expect("clarity punch");
        let (_, _, neutral) = export::render_developed_srgb_f32(
            &img,
            &DevelopRecipe {
                clarity: 0.7,
                clarity_method: ClarityMethod::Neutral,
                ..DevelopRecipe::default()
            },
            &ExportOptions::default(),
        )
        .expect("clarity neutral");
        assert_ne!(punch, neutral);

        let (_, _, soft_g) = export::render_developed_srgb_f32(
            &img,
            &DevelopRecipe {
                grain: 0.4,
                grain_type: GrainType::Soft,
                grain_size: 0.6,
                ..DevelopRecipe::default()
            },
            &ExportOptions::default(),
        )
        .expect("soft grain");
        let (_, _, harsh_g) = export::render_developed_srgb_f32(
            &img,
            &DevelopRecipe {
                grain: 0.4,
                grain_type: GrainType::Harsh,
                grain_size: 0.6,
                ..DevelopRecipe::default()
            },
            &ExportOptions::default(),
        )
        .expect("harsh grain");
        assert_ne!(soft_g, harsh_g);
    }

    #[test]
    fn phase2_export_vignette_methods_lens_keystone_and_sample_picker() {
        use rune_editor_core::{GeometryParams, LensParams, VignetteMethod};

        // 80x40 wide image cropped to right half so EllipticOnCrop, CircularOnCrop, and Circular all differ!
        let mut rgba = Vec::with_capacity(80 * 40 * 4);
        for _ in 0..(80 * 40) {
            rgba.push(f16::from_f32(0.30));
            rgba.push(f16::from_f32(0.30));
            rgba.push(f16::from_f32(0.30));
            rgba.push(f16::ONE);
        }
        let img = LinearImage {
            width: 80,
            height: 40,
            histogram_proxy: build_histogram_proxy(&rgba, 80, 40),
            rgba16f: rgba,
            display_name: "VignetteTest.dng".to_owned(),
            source_path: None,
            format_label: "Test".to_owned(),
            exif: None,
            camera_calibration: None,
        };

        let crop = GeometryParams {
            crop: [0.5, 0.0, 0.5, 1.0],
            ..GeometryParams::default()
        };
        let (_, _, ell) = export::render_developed_srgb_f32(
            &img,
            &DevelopRecipe {
                vignette: -0.8,
                vignette_method: VignetteMethod::EllipticOnCrop,
                geometry: crop,
                ..DevelopRecipe::default()
            },
            &ExportOptions::default(),
        )
        .unwrap();
        let (_, _, circ_full) = export::render_developed_srgb_f32(
            &img,
            &DevelopRecipe {
                vignette: -0.8,
                vignette_method: VignetteMethod::Circular,
                geometry: crop,
                ..DevelopRecipe::default()
            },
            &ExportOptions::default(),
        )
        .unwrap();
        // Left edge of right-half crop is at full-image center (ori_u=0.5), so Circular keeps it bright while EllipticOnCrop darkens it!
        let left_mid_idx = 20 * 40 + 1;
        assert!(
            circ_full[left_mid_idx][0] > ell[left_mid_idx][0] + 0.05,
            "Circular vignette should stay centered on full frame: circ={} vs ell={}",
            circ_full[left_mid_idx][0],
            ell[left_mid_idx][0]
        );

        // Optical light falloff brightens corners relative to center.
        let (_, _, lf) = export::render_developed_srgb_f32(
            &img,
            &DevelopRecipe {
                lens: LensParams {
                    light_falloff: 0.8,
                    ..LensParams::default()
                },
                ..DevelopRecipe::default()
            },
            &ExportOptions::default(),
        )
        .unwrap();
        assert!(lf[0][0] > lf[20 * 80 + 40][0] + 0.05);

        // sample_linear_rgb_at_uv evaluates Stages 1..=8 (exposure/WB) so pickers match Stage 8b/8c,
        // and checks bounds *after* negative radial distortion pulls rotated edge UVs back inside [0, 1]^2.
        let sampled = sample_linear_rgb_at_uv(&img, 0.5, 0.5, &DevelopRecipe::default());
        assert!((sampled[0] - 0.30).abs() < 1e-3);

        let graded_recipe = DevelopRecipe {
            exposure: 1.0,
            temperature: 0.4,
            ..DevelopRecipe::default()
        };
        let graded_sample = sample_linear_rgb_at_uv(&img, 0.5, 0.5, &graded_recipe);
        assert!(
            graded_sample[0] > sampled[0] * 2.0 && graded_sample[0] > graded_sample[2] * 1.2,
            "sample_linear_rgb_at_uv should reflect Stage 1..=8 exposure & WB: {graded_sample:?}"
        );

        let rot_neg_dist = DevelopRecipe {
            geometry: GeometryParams {
                straighten_deg: 15.0,
                ..GeometryParams::default()
            },
            lens: LensParams {
                distortion: -0.9,
                ..LensParams::default()
            },
            ..DevelopRecipe::default()
        };
        // Corner UV pulled outside [0,1] by straighten_deg=15° is brought back inside by distortion=-0.9!
        let corner_sample = sample_linear_rgb_at_uv(&img, 0.04, 0.04, &rot_neg_dist);
        assert!(
            corner_sample[0] > 0.25,
            "corner pulled back inside [0,1] by negative distortion should sample valid pixel, got {corner_sample:?}"
        );
    }

    #[test]
    fn phase3_naming_tokens_watermark_display_p3_and_multi_recipe_export() {
        // 1. Dynamic naming tokens + traversal sanitization.
        let ctx = NamingContext {
            image_name: "DSC_0421".to_owned(),
            counter: 7,
            date: "2026-11-26".to_owned(),
            camera: "Leica Q3".to_owned(),
            lens: "28mm/f1.7".to_owned(),
            iso: "ISO400".to_owned(),
            variant_index: 1,
            recipe_name: "Web 2048px".to_owned(),
        };
        let expanded = expand_naming_template(
            "{Date}_{ImageName}_{Counter}_{Variant}_{Camera}_{Lens}_{ISO}_{Recipe}",
            &ctx,
        );
        assert_eq!(
            expanded,
            "2026-11-26_DSC_0421_0007_V2_Leica_Q3_28mm_f1.7_ISO400_Web_2048px"
        );
        let hostile = expand_naming_template("../../etc/passwd\0..\\\\{ImageName}", &ctx);
        assert!(!hostile.contains(".."));
        assert!(!hostile.contains('/'));
        assert!(!hostile.contains('\\'));
        assert!(!hostile.starts_with('.'));
        assert!(hostile.contains("DSC_0421"));

        // 2. Display P3 linear matrix preserves neutral gray and compresses wide-gamut sRGB red cleanly.
        let gray_p3 = linear_srgb_to_display_p3([0.5, 0.5, 0.5]);
        assert!((gray_p3[0] - 0.5).abs() < 1e-5);
        assert!((gray_p3[1] - 0.5).abs() < 1e-5);
        assert!((gray_p3[2] - 0.5).abs() < 1e-5);
        let red_p3 = linear_srgb_to_display_p3([1.0, 0.0, 0.0]);
        assert!((red_p3[0] - 0.822462).abs() < 1e-4 && red_p3[1] > 0.03);

        // 3. Text Watermark + Display P3 rendering on a solid image.
        let w = 160u32;
        let h = 100u32;
        let mut rgba16f = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            rgba16f.push(f16::from_f32(0.40));
            rgba16f.push(f16::from_f32(0.08));
            rgba16f.push(f16::from_f32(0.06));
            rgba16f.push(f16::from_f32(1.0));
        }
        let img = LinearImage {
            width: w,
            height: h,
            rgba16f,
            histogram_proxy: vec![[0.40, 0.08, 0.06]; 16],
            display_name: "p3_wm.dng".to_owned(),
            source_path: None,
            format_label: "RAW".to_owned(),
            exif: None,
            camera_calibration: None,
        };
        let (_, _, srgb_plain) = export::render_developed_srgb_f32(
            &img,
            &DevelopRecipe::default(),
            &ExportOptions::default(),
        )
        .unwrap();
        let (_, _, p3_plain) = export::render_developed_srgb_f32(
            &img,
            &DevelopRecipe::default(),
            &ExportOptions {
                color_space: ExportColorSpace::DisplayP3,
                ..ExportOptions::default()
            },
        )
        .unwrap();
        assert!(
            p3_plain[0][0] < srgb_plain[0][0] - 0.02 && p3_plain[0][1] > srgb_plain[0][1] + 0.01,
            "Display P3 export should apply chromaticity transform: p3={:?} srgb={:?}",
            p3_plain[0],
            srgb_plain[0]
        );

        let wm_opts = ExportOptions {
            watermark: TextWatermark {
                enabled: true,
                text: "© RUNE STUDIO".to_owned(),
                opacity: 0.75,
                scale: 1.5,
                position: WatermarkPosition::Center,
            },
            ..ExportOptions::default()
        };
        let (_, _, srgb_wm) =
            export::render_developed_srgb_f32(&img, &DevelopRecipe::default(), &wm_opts).unwrap();
        let changed = srgb_plain
            .iter()
            .zip(srgb_wm.iter())
            .filter(|(a, b)| (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs() > 0.05)
            .count();
        assert!(
            changed >= 40,
            "TextWatermark should visibly composite glyphs onto image (changed {changed} pixels)"
        );

        // 4. Multi-Recipe Export: pre-flight collision, subfolder traversal, guard_not_source, overwrite=false, and success.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "rune_decode_multi_recipe_{}_{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let mut recipes = builtin_export_recipes();
        recipes[2].enabled = true; // Enable all 3 recipes (Full-Res JPEG, Web 2048px, Archival 16-bit TIFF)
        let exported = export_multi_recipe(
            &img,
            &DevelopRecipe::default(),
            &dir,
            &recipes,
            &ctx,
            None,
            false,
        )
        .expect("multi-recipe export should succeed");
        assert_eq!(exported.len(), 3);
        for item in &exported {
            assert!(item.path.exists(), "missing exported file {:?}", item.path);
            assert_eq!(item.width, w);
            assert_eq!(item.height, h);
        }

        // Re-running with overwrite=false must fail with DestinationExists before writing.
        let err_exists = export_multi_recipe(
            &img,
            &DevelopRecipe::default(),
            &dir,
            &recipes,
            &ctx,
            None,
            false,
        );
        assert!(matches!(err_exists, Err(DecodeError::DestinationExists(_))));

        // Duplicate destination collision check must fail pre-flight.
        let dup_recipes = vec![
            ExportRecipeSpec {
                name: "Same".to_owned(),
                enabled: true,
                subfolder: String::new(),
                naming_template: "{ImageName}".to_owned(),
                options: ExportOptions::default(),
            },
            ExportRecipeSpec {
                name: "Same2".to_owned(),
                enabled: true,
                subfolder: String::new(),
                naming_template: "{ImageName}".to_owned(),
                options: ExportOptions::default(),
            },
        ];
        let err_dup = plan_multi_recipe_paths(&dir, &dup_recipes, &ctx, None, true);
        assert!(matches!(err_dup, Err(DecodeError::DuplicateDestination(_))));

        // Traversal subfolder must be rejected pre-flight.
        let bad_sub = vec![ExportRecipeSpec {
            name: "Escape".to_owned(),
            enabled: true,
            subfolder: "../escape".to_owned(),
            naming_template: "{ImageName}".to_owned(),
            options: ExportOptions::default(),
        }];
        assert!(matches!(
            plan_multi_recipe_paths(&dir, &bad_sub, &ctx, None, true),
            Err(DecodeError::InvalidPath(_))
        ));

        // guard_not_source must block overwriting the source photo.
        let src_collision = dir.join("DSC_0421.jpg");
        std::fs::write(&src_collision, b"original").unwrap();
        let single = vec![ExportRecipeSpec {
            name: "Orig".to_owned(),
            enabled: true,
            subfolder: String::new(),
            naming_template: "{ImageName}".to_owned(),
            options: ExportOptions::default(),
        }];
        assert!(matches!(
            plan_multi_recipe_paths(&dir, &single, &ctx, Some(&src_collision), true),
            Err(DecodeError::SourceOverwrite(_))
        ));

        // Tiny 4x4 watermarked image must export cleanly without clamp min > max panic.
        let tiny_img = LinearImage {
            width: 4,
            height: 4,
            rgba16f: vec![f16::from_f32(0.25); 4 * 4 * 4],
            histogram_proxy: vec![[0.25, 0.25, 0.25]; 4],
            display_name: "tiny.dng".to_owned(),
            source_path: None,
            format_label: "RAW".to_owned(),
            exif: None,
            camera_calibration: None,
        };
        let tiny_out = dir.join("tiny_wm.png");
        let tiny_opts = ExportOptions {
            format: ExportFormat::Png8,
            watermark: TextWatermark {
                enabled: true,
                text: "RUNE".to_owned(),
                opacity: 0.6,
                scale: 1.0,
                position: WatermarkPosition::BottomRight,
            },
            ..ExportOptions::default()
        };
        export_image_with_options(&tiny_img, &DevelopRecipe::default(), &tiny_out, &tiny_opts)
            .expect("4x4 watermarked export must not panic");
        assert!(tiny_out.exists());

        // Case-insensitive destination collision check on macOS/Windows (Photo_Web.jpg vs photo_web.jpg).
        if cfg!(any(target_os = "macos", target_os = "windows")) {
            let case_dup = vec![
                ExportRecipeSpec {
                    name: "Web".to_owned(),
                    enabled: true,
                    subfolder: String::new(),
                    naming_template: "Photo_{Recipe}".to_owned(),
                    options: ExportOptions::default(),
                },
                ExportRecipeSpec {
                    name: "web".to_owned(),
                    enabled: true,
                    subfolder: String::new(),
                    naming_template: "photo_{Recipe}".to_owned(),
                    options: ExportOptions::default(),
                },
            ];
            assert!(matches!(
                plan_multi_recipe_paths(&dir, &case_dup, &ctx, None, true),
                Err(DecodeError::DuplicateDestination(_))
            ));
        }

        // Dangling symlink at destination must be caught by overwrite=false pre-flight check.
        #[cfg(unix)]
        {
            let dangling = dir.join("dangling_out.jpg");
            std::os::unix::fs::symlink(dir.join("nonexistent_target.jpg"), &dangling)
                .expect("create dangling symlink");
            assert!(!dangling.exists() && dangling.symlink_metadata().is_ok());
            let dangling_spec = vec![ExportRecipeSpec {
                name: "Dangling".to_owned(),
                enabled: true,
                subfolder: String::new(),
                naming_template: "dangling_out".to_owned(),
                options: ExportOptions::default(),
            }];
            assert!(matches!(
                plan_multi_recipe_paths(&dir, &dangling_spec, &ctx, None, false),
                Err(DecodeError::DestinationExists(_))
            ));
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn phase4_ai_subject_mask_layers_and_heal_clone_export() {
        use rune_editor_core::{
            AdjustmentLayer, BrushDab, HealMode, HealSpot, LayerMaskItem, LocalAdjustments,
            MaskCombineOp, MaskShape,
        };

        // 1. AI Subject / Background mask on a bright warm subject centered over a dark cool backdrop.
        let w = 128u32;
        let h = 128u32;
        let mut rgba16f = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let dx = (x as f32 + 0.5) / w as f32 - 0.5;
                let dy = (y as f32 + 0.5) / h as f32 - 0.5;
                let in_subject = dx * dx + dy * dy < 0.22 * 0.22;
                let rgb = if in_subject {
                    [0.72_f32, 0.42, 0.18]
                } else {
                    [0.03_f32, 0.06, 0.14]
                };
                rgba16f.push(f16::from_f32(rgb[0]));
                rgba16f.push(f16::from_f32(rgb[1]));
                rgba16f.push(f16::from_f32(rgb[2]));
                rgba16f.push(f16::ONE);
            }
        }
        let img = LinearImage {
            width: w,
            height: h,
            rgba16f,
            histogram_proxy: vec![[0.2, 0.2, 0.2]; 16],
            display_name: "subject.dng".to_owned(),
            source_path: None,
            format_label: "RAW".to_owned(),
            exif: None,
            camera_calibration: None,
        };

        let subj_mask = generate_subject_mask(&img, false);
        let bg_mask = generate_subject_mask(&img, true);
        let (sw, sh, subj_bytes) = subj_mask.decode_raster_u8().expect("decode subject mask");
        let (bw, bh, bg_bytes) = bg_mask.decode_raster_u8().expect("decode bg mask");
        let tile = rune_editor_core::LAYER_MASK_TILE_SIZE as u32;
        assert_eq!((sw, sh), (tile, tile));
        assert_eq!((bw, bh), (tile, tile));

        // Exact complement between subject and background masks.
        for (&s, &b) in subj_bytes.iter().zip(bg_bytes.iter()) {
            assert_eq!(s as u16 + b as u16, 255);
        }
        let tile_usize = rune_editor_core::LAYER_MASK_TILE_SIZE;
        let center_val = subj_bytes[(tile_usize / 2) * tile_usize + (tile_usize / 2)];
        let corner_val = subj_bytes[16 * tile_usize + 16];
        assert!(
            center_val > 200 && corner_val < 40,
            "AI Subject mask should isolate center subject ({center_val}) vs dark corner ({corner_val})"
        );

        // 2. Spot Heal matches surrounding luminance while Clone transfers donor pixels directly.
        let mut blemish_rgba = vec![f16::ZERO; (64 * 64 * 4) as usize];
        for y in 0..64u32 {
            for x in 0..64u32 {
                // Left half bright gray (0.40), right half darker gray (0.16), dark blemish dot at (16, 32).
                let mut v = if x < 32 { 0.40_f32 } else { 0.16_f32 };
                if (x as i32 - 16).abs() <= 2 && (y as i32 - 32).abs() <= 2 {
                    v = 0.02;
                }
                let base = ((y * 64 + x) * 4) as usize;
                blemish_rgba[base] = f16::from_f32(v);
                blemish_rgba[base + 1] = f16::from_f32(v);
                blemish_rgba[base + 2] = f16::from_f32(v);
                blemish_rgba[base + 3] = f16::ONE;
            }
        }
        let blemish_img = LinearImage {
            width: 64,
            height: 64,
            rgba16f: blemish_rgba,
            histogram_proxy: vec![[0.25, 0.25, 0.25]; 16],
            display_name: "blemish.dng".to_owned(),
            source_path: None,
            format_label: "RAW".to_owned(),
            exif: None,
            camera_calibration: None,
        };
        let clone_recipe = DevelopRecipe {
            heal_spots: vec![HealSpot {
                id: "s1".to_owned(),
                target: [0.25, 0.5],
                source: [0.75, 0.5],
                radius: 0.08,
                feather: 0.2,
                opacity: 1.0,
                mode: HealMode::Clone,
            }],
            ..DevelopRecipe::default()
        };
        let heal_recipe = DevelopRecipe {
            heal_spots: vec![HealSpot {
                id: "s1".to_owned(),
                target: [0.25, 0.5],
                source: [0.75, 0.5],
                radius: 0.08,
                feather: 0.2,
                opacity: 1.0,
                mode: HealMode::Heal,
            }],
            ..DevelopRecipe::default()
        };
        let (_, _, cloned) = export::render_developed_srgb_f32(
            &blemish_img,
            &clone_recipe,
            &ExportOptions::default(),
        )
        .unwrap();
        let (_, _, healed) = export::render_developed_srgb_f32(
            &blemish_img,
            &heal_recipe,
            &ExportOptions::default(),
        )
        .unwrap();
        let blemish_px = 32 * 64 + 16;
        let clean_surr = 32 * 64 + 8;
        assert!(
            (healed[blemish_px][0] - healed[clean_surr][0]).abs() < 0.04,
            "Heal should match target perimeter brightness: healed={} vs surround={}",
            healed[blemish_px][0],
            healed[clean_surr][0]
        );
        assert!(
            cloned[blemish_px][0] < healed[blemish_px][0] - 0.12,
            "Clone from darker donor should be darker than brightness-matched Heal: clone={} vs heal={}",
            cloned[blemish_px][0],
            healed[blemish_px][0]
        );

        // 3. Combined Linear + Brush adjustment layer brightens masked region without touching unmasked region.
        let mut layer = AdjustmentLayer::new("l1", "Spotlight");
        layer.masks.push(LayerMaskItem {
            enabled: true,
            op: MaskCombineOp::Add,
            inverted: false,
            shape: MaskShape::BrushStrokes {
                strokes: vec![BrushDab {
                    x: 0.5,
                    y: 0.5,
                    radius: 0.2,
                    hardness: 0.8,
                    opacity: 1.0,
                    erase: false,
                }],
            },
        });
        layer.adjustments = LocalAdjustments {
            exposure: 1.5,
            ..LocalAdjustments::default()
        };
        let (_, _, plain) = export::render_developed_srgb_f32(
            &img,
            &DevelopRecipe::default(),
            &ExportOptions::default(),
        )
        .unwrap();
        let (_, _, layered) = export::render_developed_srgb_f32(
            &img,
            &DevelopRecipe {
                layers: vec![layer],
                ..DevelopRecipe::default()
            },
            &ExportOptions::default(),
        )
        .unwrap();
        let center_idx = (64 * 128 + 64) as usize;
        let corner_idx = (4 * 128 + 4) as usize;
        assert!(layered[center_idx][0] > plain[center_idx][0] + 0.08);
        assert!((layered[corner_idx][0] - plain[corner_idx][0]).abs() < 1e-4);
    }

    #[test]
    fn phase5_hot_folder_watcher_stability_and_raw_film_base_sample() {
        let dir = unique_temp_dir("hot_folder_p5");

        // Pre-existing photo should be seeded as known by HotFolderWatcher::new.
        let existing = dir.join("existing.jpg");
        fs::write(&existing, b"already-here").unwrap();
        let mut watcher = HotFolderWatcher::new(dir.clone()).expect("init hot folder watcher");
        assert_eq!(watcher.known_count(), 1);
        assert!(watcher.poll_ready().unwrap().is_empty());

        // Simulate a camera/tether arrival: empty file -> growing -> stable across 2 polls.
        let incoming = dir.join("shot_001.jpg");
        fs::write(&incoming, b"").unwrap();
        assert!(
            watcher.poll_ready().unwrap().is_empty(),
            "0-byte file must wait"
        );

        fs::write(&incoming, b"part1").unwrap();
        assert!(
            watcher.poll_ready().unwrap().is_empty(),
            "first non-zero poll records size"
        );

        fs::write(&incoming, b"part1+part2").unwrap();
        assert!(
            watcher.poll_ready().unwrap().is_empty(),
            "growing file must not emit yet"
        );

        // Hidden dotfile and symlink must be ignored.
        fs::write(dir.join(".tmp_shot.jpg"), b"hidden").unwrap();
        #[cfg(unix)]
        {
            let _ = std::os::unix::fs::symlink(&existing, dir.join("link_shot.jpg"));
        }

        // Next poll sees stable byte size -> emits shot_001.jpg once and never again.
        let ready = watcher.poll_ready().unwrap();
        assert_eq!(ready, vec![incoming.clone()]);
        assert!(watcher.poll_ready().unwrap().is_empty());

        // HotFolderWatcher::new_empty picks up existing files after 2 stability polls (for `--once`).
        let mut empty_w = HotFolderWatcher::new_empty(dir.clone()).unwrap();
        assert!(empty_w.poll_ready().unwrap().is_empty());
        let all_ready = empty_w.poll_ready().unwrap();
        assert_eq!(all_ready, vec![existing, incoming]);

        // sample_raw_linear_rgb_at_uv returns raw un-inverted film rebate even when FilmNegative is active.
        let img = LinearImage {
            width: 8,
            height: 8,
            rgba16f: [
                f16::from_f32(0.90),
                f16::from_f32(0.54),
                f16::from_f32(0.32),
                f16::ONE,
            ]
            .repeat(64),
            histogram_proxy: vec![[0.90, 0.54, 0.32]; 8],
            display_name: "neg.dng".to_owned(),
            source_path: None,
            format_label: "RAW".to_owned(),
            exif: None,
            camera_calibration: None,
        };
        let mut neg_recipe = DevelopRecipe::default();
        neg_recipe.convert_film_negative_from_base([0.90, 0.54, 0.32]);
        let raw_px = sample_raw_linear_rgb_at_uv(&img, 0.5, 0.5, &neg_recipe);
        let dev_px = sample_linear_rgb_at_uv(&img, 0.5, 0.5, &neg_recipe);
        assert!((raw_px[0] - 0.90).abs() < 2e-3 && (raw_px[2] - 0.32).abs() < 2e-3);
        assert!(dev_px[0] < 1e-3 && dev_px[1] < 1e-3 && dev_px[2] < 1e-3);

        let _ = fs::remove_dir_all(&dir);
    }
}
