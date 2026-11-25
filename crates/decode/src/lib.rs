//! Image decoding, Camera RAW / ProRAW / HEIC ingestion, folder scanning,
//! fast filmstrip thumbnails, and full-resolution photo export.
//!
//! Converts RAW and standard photo formats into 16-bit half-float scene-referred
//! linear `Rgba16Float` buffers ready for direct upload into the GPU develop
//! pipeline, plus downsampled proxies for sub-millisecond live histogram evaluation.

pub mod export;
pub mod folder;
pub mod raw;

use std::path::{Path, PathBuf};

use half::f16;
use thiserror::Error;

pub use export::{ExportFormat, export_image};
pub use folder::{ThumbnailRgba8, generate_demo_thumbnail, load_thumbnail, scan_photo_folder};
pub use raw::{
    RASTER_EXTENSIONS, RAW_EXTENSIONS, SUPPORTED_EXTENSIONS, is_raw_extension, is_supported_photo,
    load_file_proxy,
};

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

    Ok(LinearImage {
        width,
        height,
        rgba16f,
        histogram_proxy,
        display_name: display_name.to_owned(),
        source_path: None,
        format_label: format_label.to_owned(),
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
}
