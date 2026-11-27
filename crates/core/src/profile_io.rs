//! External industry camera profile parsers (`.cube`, `.dcp`, `.icc`/`.icm`)
//! and safe user profile store (`~/.config/rune-darkroom/profiles`).
//!
//! All binary and text parsers are pure Rust with zero external crates, strict file-size
//! (`16 MiB`) and dimension caps, checked integer arithmetic, finite-float validation,
//! symlink/traversal guards, and atomic persistence (`temp dotfile -> sync_all -> rename`).

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{
    CoreError,
    curves::MonotonicSpline,
    hsv_to_rgb,
    profiles::{
        CameraProfileDescriptor, PROFILE_LUT_SIZE, ProfileCategory, ProfileLut3D,
        bradford_adaptation_matrix, builtin_camera_profiles, cached_builtin_profile_lut,
    },
    rgb_to_hsv,
    session::atomic_write_bytes,
    srgb_to_linear,
};

/// Maximum external profile file size accepted by all parsers and the user store (`16 MiB`).
pub const MAX_PROFILE_FILE_BYTES: usize = 16 * 1024 * 1024;

/// Maximum edge dimension accepted for `.cube` 3D LUTs (`65×65×65`).
pub const MAX_CUBE_LUT_SIZE: usize = 65;

/// Maximum hue divisions in `.dcp` `HueSatMap` / `LookTable` tables.
pub const MAX_DCP_HUE_DIVS: usize = 180;

/// Maximum saturation divisions in `.dcp` `HueSatMap` / `LookTable` tables.
pub const MAX_DCP_SAT_DIVS: usize = 128;

/// Maximum value divisions in `.dcp` `HueSatMap` / `LookTable` tables.
pub const MAX_DCP_VAL_DIVS: usize = 64;

/// Maximum TIFF IFD entries scanned in a `.dcp` container.
pub const MAX_DCP_IFD_ENTRIES: usize = 512;

/// Maximum control points in a `.dcp` `ProfileToneCurve`.
pub const MAX_DCP_TONE_CURVE_POINTS: usize = 16_384;

/// Maximum tag records accepted in an `.icc`/`.icm` profile header.
pub const MAX_ICC_TAG_COUNT: usize = 256;

/// Maximum 3D CLUT grid edge dimension accepted in ICC `mft1` / `mft2` tags (`65×65×65`).
pub const MAX_ICC_CLUT_GRID: usize = 65;

/// Maximum entries accepted in an ICC `curv` 1D LUT tag.
pub const MAX_ICC_CURVE_ENTRIES: usize = 65_536;

/// Prefix used in `DevelopRecipe.camera_profile` for user-imported profiles (`"user:<slug>"`).
pub const USER_PROFILE_PREFIX: &str = "user:";

/// Strictly allowlisted file extensions for imported camera profiles (lowercase).
pub const ALLOWED_PROFILE_EXTENSIONS: &[&str] = &["dcp", "cube", "icc", "icm"];

/// Maximum cached user profile 3D LUTs kept in memory.
const MAX_CACHED_USER_LUTS: usize = 64;

// Exact IEC 61966-2-1 Linear sRGB (D65) -> ProPhoto / ROMM RGB (D50, Bradford-adapted).
// Row sums equal 1.000000 for exact neutral gray preservation.
const SRGB_D65_TO_PROPHOTO_D50: [f32; 9] = [
    0.529_345_9,
    0.330_072_8,
    0.140_581_3, //
    0.098_374_4,
    0.873_461_1,
    0.028_164_7, //
    0.016_883_2,
    0.117_672_5,
    0.865_444_3,
];

// Exact inverse: ProPhoto / ROMM RGB (D50) -> Linear sRGB (D65, Bradford-adapted).
const PROPHOTO_D50_TO_SRGB_D65: [f32; 9] = [
    2.034_076_2,
    -0.727_334_3,
    -0.306_741_7, //
    -0.228_813_4,
    1.231_730_2,
    -0.002_916_9, //
    -0.008_569_8,
    -0.153_286_6,
    1.161_856_4,
];

const D50_XYZ: [f32; 3] = [0.96422, 1.00000, 0.82521];
const D65_XYZ: [f32; 3] = [0.95047, 1.00000, 1.08883];

const XYZ_D65_TO_SRGB: [f32; 9] = [
    3.240_454_2,
    -1.537_138_5,
    -0.498_531_4, //
    -0.969_266,
    1.876_010_8,
    0.041_556, //
    0.055_643_4,
    -0.204_025_9,
    1.057_225_2,
];

#[inline]
fn mul_mat3_vec3(m: &[f32; 9], v: [f32; 3]) -> [f32; 3] {
    [
        m[0] * v[0] + m[1] * v[1] + m[2] * v[2],
        m[3] * v[0] + m[4] * v[1] + m[5] * v[2],
        m[6] * v[0] + m[7] * v[1] + m[8] * v[2],
    ]
}

#[inline]
fn mul_mat3(a: &[f32; 9], b: &[f32; 9]) -> [f32; 9] {
    let mut out = [0.0_f32; 9];
    for r in 0..3 {
        for c in 0..3 {
            out[r * 3 + c] = a[r * 3] * b[c] + a[r * 3 + 1] * b[3 + c] + a[r * 3 + 2] * b[6 + c];
        }
    }
    out
}

#[inline]
fn linear_to_srgb_exact(x: f32) -> f32 {
    let c = if x.is_finite() {
        x.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

#[inline]
fn xyz_d50_to_linear_srgb_matrix() -> [f32; 9] {
    let d50_to_d65 = bradford_adaptation_matrix(D50_XYZ, D65_XYZ);
    mul_mat3(&XYZ_D65_TO_SRGB, &d50_to_d65)
}

// ── 1. Pure-Rust `.cube` 3D LUT Parser ───────────────────────────────────────

/// Parsed Adobe / DaVinci `.cube` 3D LUT in encoded `[0, 1]` display/grading domain.
#[derive(Clone, Debug, PartialEq)]
pub struct CubeLut3D {
    pub title: Option<String>,
    pub size: usize,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    /// R-fastest triplets (`size^3` entries, index `(b * size + g) * size + r`).
    pub data: Vec<[f32; 3]>,
}

/// Parses an Adobe / DaVinci `.cube` 3D LUT file with strict bounds and finiteness checks.
pub fn parse_cube_lut(bytes: &[u8]) -> Result<CubeLut3D, CoreError> {
    if bytes.is_empty() {
        return Err(CoreError::Validation("Empty .cube file".to_owned()));
    }
    if bytes.len() > MAX_PROFILE_FILE_BYTES {
        return Err(CoreError::Validation(format!(
            ".cube file exceeds {} MiB cap",
            MAX_PROFILE_FILE_BYTES / (1024 * 1024)
        )));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| CoreError::Validation(".cube file must be valid UTF-8 text".to_owned()))?;
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);

    let mut title: Option<String> = None;
    let mut size: Option<usize> = None;
    let mut domain_min = [0.0_f32; 3];
    let mut domain_max = [1.0_f32; 3];
    let mut data: Vec<[f32; 3]> = Vec::new();
    let mut max_entries: Option<usize> = None;

    for (line_idx, raw_line) in text.lines().enumerate() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Handle TITLE before stripping inline `#` so quoted titles may contain `#`.
        let upper_prefix = trimmed
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        if upper_prefix == "TITLE" {
            let rest = trimmed[5..].trim();
            let parsed_title = if let Some(after_q) = rest.strip_prefix('"') {
                after_q.split('"').next().unwrap_or("").trim().to_owned()
            } else {
                rest.split('#').next().unwrap_or("").trim().to_owned()
            };
            if !parsed_title.is_empty() {
                title = Some(parsed_title);
            }
            continue;
        }

        let line = trimmed.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.is_empty() {
            continue;
        }

        match tokens[0].to_ascii_uppercase().as_str() {
            "LUT_3D_SIZE" => {
                if size.is_some() {
                    return Err(CoreError::Validation(
                        "Duplicate LUT_3D_SIZE directive in .cube file".to_owned(),
                    ));
                }
                if tokens.len() != 2 {
                    return Err(CoreError::Validation(format!(
                        "Invalid LUT_3D_SIZE directive on line {}",
                        line_idx + 1
                    )));
                }
                let n: usize = tokens[1].parse().map_err(|_| {
                    CoreError::Validation(format!(
                        "Invalid LUT_3D_SIZE '{}' on line {}",
                        tokens[1],
                        line_idx + 1
                    ))
                })?;
                if !(2..=MAX_CUBE_LUT_SIZE).contains(&n) {
                    return Err(CoreError::Validation(format!(
                        "LUT_3D_SIZE {n} out of supported range 2..={MAX_CUBE_LUT_SIZE}"
                    )));
                }
                let total = n
                    .checked_mul(n)
                    .and_then(|v| v.checked_mul(n))
                    .ok_or_else(|| CoreError::Validation("LUT_3D_SIZE overflow".to_owned()))?;
                size = Some(n);
                max_entries = Some(total);
                data.reserve(total);
            }
            "LUT_1D_SIZE" => {
                return Err(CoreError::Validation(
                    "1D .cube LUTs (LUT_1D_SIZE) are not supported; provide a 3D LUT_3D_SIZE .cube"
                        .to_owned(),
                ));
            }
            "DOMAIN_MIN" => {
                domain_min = parse_cube_vec3(&tokens[1..], "DOMAIN_MIN", line_idx + 1)?;
            }
            "DOMAIN_MAX" => {
                domain_max = parse_cube_vec3(&tokens[1..], "DOMAIN_MAX", line_idx + 1)?;
            }
            "LUT_3D_INPUT_RANGE" => {
                if tokens.len() != 3 {
                    return Err(CoreError::Validation(format!(
                        "Invalid LUT_3D_INPUT_RANGE on line {}",
                        line_idx + 1
                    )));
                }
                let lo = parse_finite_cube_f32(tokens[1], line_idx + 1)?;
                let hi = parse_finite_cube_f32(tokens[2], line_idx + 1)?;
                domain_min = [lo, lo, lo];
                domain_max = [hi, hi, hi];
            }
            _ => {
                if tokens.len() != 3 {
                    return Err(CoreError::Validation(format!(
                        "Unexpected line {} in .cube file: '{}'",
                        line_idx + 1,
                        line.chars().take(64).collect::<String>()
                    )));
                }
                let r = parse_finite_cube_f32(tokens[0], line_idx + 1)?;
                let g = parse_finite_cube_f32(tokens[1], line_idx + 1)?;
                let b = parse_finite_cube_f32(tokens[2], line_idx + 1)?;
                let cap = max_entries
                    .unwrap_or(MAX_CUBE_LUT_SIZE * MAX_CUBE_LUT_SIZE * MAX_CUBE_LUT_SIZE);
                if data.len() >= cap {
                    return Err(CoreError::Validation(format!(
                        ".cube file contains more than {cap} RGB triplets"
                    )));
                }
                data.push([r, g, b]);
            }
        }
    }

    let size = size.ok_or_else(|| {
        CoreError::Validation("Missing LUT_3D_SIZE directive in .cube file".to_owned())
    })?;
    for ch in 0..3 {
        if !domain_min[ch].is_finite()
            || !domain_max[ch].is_finite()
            || domain_max[ch] <= domain_min[ch] + 1e-6
        {
            return Err(CoreError::Validation(format!(
                "Invalid .cube domain on channel {ch}: min={} max={}",
                domain_min[ch], domain_max[ch]
            )));
        }
    }
    let expected = size * size * size;
    if data.len() != expected {
        return Err(CoreError::Validation(format!(
            ".cube triplet count mismatch: expected {expected} ({size}^3), found {}",
            data.len()
        )));
    }

    Ok(CubeLut3D {
        title,
        size,
        domain_min,
        domain_max,
        data,
    })
}

fn parse_finite_cube_f32(tok: &str, line: usize) -> Result<f32, CoreError> {
    let v: f32 = tok.parse().map_err(|_| {
        CoreError::Validation(format!("Invalid float '{tok}' on .cube line {line}"))
    })?;
    if !v.is_finite() {
        return Err(CoreError::Validation(format!(
            "Non-finite float '{tok}' on .cube line {line}"
        )));
    }
    Ok(v)
}

fn parse_cube_vec3(tokens: &[&str], key: &str, line: usize) -> Result<[f32; 3], CoreError> {
    if tokens.len() != 3 {
        return Err(CoreError::Validation(format!(
            "{key} expects 3 floats on line {line}"
        )));
    }
    Ok([
        parse_finite_cube_f32(tokens[0], line)?,
        parse_finite_cube_f32(tokens[1], line)?,
        parse_finite_cube_f32(tokens[2], line)?,
    ])
}

impl CubeLut3D {
    /// Trilinearly samples the `.cube` LUT in its native encoded domain (`domain_min..domain_max`).
    #[inline]
    pub fn sample_encoded(&self, rgb: [f32; 3]) -> [f32; 3] {
        let n = self.size;
        if n < 2 || self.data.len() < n * n * n {
            return rgb;
        }
        let max_i = (n - 1) as f32;
        let norm = |v: f32, ch: usize| -> f32 {
            let span = (self.domain_max[ch] - self.domain_min[ch]).max(1e-6);
            ((v - self.domain_min[ch]) / span).clamp(0.0, 1.0)
        };
        let fx = norm(rgb[0], 0) * max_i;
        let fy = norm(rgb[1], 1) * max_i;
        let fz = norm(rgb[2], 2) * max_i;

        let x0 = (fx.floor() as usize).min(n - 1);
        let y0 = (fy.floor() as usize).min(n - 1);
        let z0 = (fz.floor() as usize).min(n - 1);
        let x1 = (x0 + 1).min(n - 1);
        let y1 = (y0 + 1).min(n - 1);
        let z1 = (z0 + 1).min(n - 1);

        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let tz = fz - z0 as f32;

        let stride_y = n;
        let stride_z = n * n;
        let c000 = self.data[z0 * stride_z + y0 * stride_y + x0];
        let c100 = self.data[z0 * stride_z + y0 * stride_y + x1];
        let c010 = self.data[z0 * stride_z + y1 * stride_y + x0];
        let c110 = self.data[z0 * stride_z + y1 * stride_y + x1];
        let c001 = self.data[z1 * stride_z + y0 * stride_y + x0];
        let c101 = self.data[z1 * stride_z + y0 * stride_y + x1];
        let c011 = self.data[z1 * stride_z + y1 * stride_y + x0];
        let c111 = self.data[z1 * stride_z + y1 * stride_y + x1];

        let mut out = [0.0_f32; 3];
        for ch in 0..3 {
            let c00 = c000[ch] + (c100[ch] - c000[ch]) * tx;
            let c10 = c010[ch] + (c110[ch] - c010[ch]) * tx;
            let c01 = c001[ch] + (c101[ch] - c001[ch]) * tx;
            let c11 = c011[ch] + (c111[ch] - c011[ch]) * tx;
            let c0 = c00 + (c10 - c00) * ty;
            let c1 = c01 + (c11 - c01) * ty;
            out[ch] = c0 + (c1 - c0) * tz;
        }
        out
    }

    /// Resamples this `.cube` LUT into our unified `33³` log2-shaped linear [`ProfileLut3D`],
    /// evaluating `linear -> sRGB -> .cube -> linear` with HDR headroom (`> 1.0`) preserved smoothly.
    pub fn bake_profile_lut(&self) -> ProfileLut3D {
        ProfileLut3D::from_linear_fn(PROFILE_LUT_SIZE, |lin| {
            let sdr = [
                lin[0].clamp(0.0, 1.0),
                lin[1].clamp(0.0, 1.0),
                lin[2].clamp(0.0, 1.0),
            ];
            let hdr_excess = [
                (lin[0] - 1.0).max(0.0),
                (lin[1] - 1.0).max(0.0),
                (lin[2] - 1.0).max(0.0),
            ];
            let enc = [
                linear_to_srgb_exact(sdr[0]),
                linear_to_srgb_exact(sdr[1]),
                linear_to_srgb_exact(sdr[2]),
            ];
            let graded = self.sample_encoded(enc);
            [
                (srgb_to_linear(graded[0].clamp(0.0, 1.0)) + hdr_excess[0]).max(0.0),
                (srgb_to_linear(graded[1].clamp(0.0, 1.0)) + hdr_excess[1]).max(0.0),
                (srgb_to_linear(graded[2].clamp(0.0, 1.0)) + hdr_excess[2]).max(0.0),
            ]
        })
    }
}

// ── 2. Pure-Rust `.dcp` (Adobe DNG Camera Profile) Parser ────────────────────

/// Parsed HSV 3D adjustment table (`ProfileHueSatMapData` or `ProfileLookTableData`).
#[derive(Clone, Debug, PartialEq)]
pub struct DcpHsvTable {
    pub hue_divs: usize,
    pub sat_divs: usize,
    pub val_divs: usize,
    /// Flattened `(hue_shift_deg, sat_scale, val_scale)` triplets in DNG spec order
    /// (`val` outermost, `hue` middle, `sat` innermost).
    pub data: Vec<[f32; 3]>,
}

/// Parsed Adobe DNG Camera Profile (`.dcp`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DcpProfile {
    pub unique_camera_model: Option<String>,
    pub profile_name: Option<String>,
    pub calibration_illuminant_1: Option<u16>,
    pub calibration_illuminant_2: Option<u16>,
    pub color_matrix_1: Option<[f32; 9]>,
    pub color_matrix_2: Option<[f32; 9]>,
    pub forward_matrix_1: Option<[f32; 9]>,
    pub forward_matrix_2: Option<[f32; 9]>,
    pub hue_sat_map_1: Option<DcpHsvTable>,
    pub hue_sat_map_2: Option<DcpHsvTable>,
    pub look_table: Option<DcpHsvTable>,
    pub tone_curve: Option<Vec<[f32; 2]>>,
}

#[derive(Clone, Copy)]
enum Endian {
    Little,
    Big,
}

impl Endian {
    #[inline]
    fn u16(self, b: &[u8]) -> u16 {
        let arr = [b[0], b[1]];
        match self {
            Self::Little => u16::from_le_bytes(arr),
            Self::Big => u16::from_be_bytes(arr),
        }
    }

    #[inline]
    fn u32(self, b: &[u8]) -> u32 {
        let arr = [b[0], b[1], b[2], b[3]];
        match self {
            Self::Little => u32::from_le_bytes(arr),
            Self::Big => u32::from_be_bytes(arr),
        }
    }

    #[inline]
    fn i32(self, b: &[u8]) -> i32 {
        let arr = [b[0], b[1], b[2], b[3]];
        match self {
            Self::Little => i32::from_le_bytes(arr),
            Self::Big => i32::from_be_bytes(arr),
        }
    }

    #[inline]
    fn f32(self, b: &[u8]) -> f32 {
        let arr = [b[0], b[1], b[2], b[3]];
        match self {
            Self::Little => f32::from_le_bytes(arr),
            Self::Big => f32::from_be_bytes(arr),
        }
    }

    #[inline]
    fn f64(self, b: &[u8]) -> f64 {
        let arr = [b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]];
        match self {
            Self::Little => f64::from_le_bytes(arr),
            Self::Big => f64::from_be_bytes(arr),
        }
    }
}

/// Parses an Adobe DNG Camera Profile (`.dcp`) TIFF-IFD container (`II`/`MM`, magic `0x4352` or `0x002A`).
pub fn parse_dcp_profile(bytes: &[u8]) -> Result<DcpProfile, CoreError> {
    if bytes.len() < 8 {
        return Err(CoreError::Validation(
            ".dcp file is truncated (smaller than 8-byte TIFF header)".to_owned(),
        ));
    }
    if bytes.len() > MAX_PROFILE_FILE_BYTES {
        return Err(CoreError::Validation(format!(
            ".dcp file exceeds {} MiB cap",
            MAX_PROFILE_FILE_BYTES / (1024 * 1024)
        )));
    }

    let endian = match &bytes[0..2] {
        b"II" => Endian::Little,
        b"MM" => Endian::Big,
        _ => {
            return Err(CoreError::Validation(
                "Invalid .dcp byte order marker (expected 'II' or 'MM')".to_owned(),
            ));
        }
    };
    let magic = endian.u16(&bytes[2..4]);
    if magic != 0x4352 && magic != 0x002A {
        return Err(CoreError::Validation(format!(
            "Invalid .dcp magic 0x{magic:04X} (expected 0x4352 'CR' or 0x002A)"
        )));
    }

    let ifd_offset = endian.u32(&bytes[4..8]) as usize;
    let entries_start = ifd_offset
        .checked_add(2)
        .ok_or_else(|| CoreError::Validation(".dcp IFD offset overflow".to_owned()))?;
    let count_slice = bytes
        .get(ifd_offset..entries_start)
        .ok_or_else(|| CoreError::Validation(".dcp IFD offset out of bounds".to_owned()))?;
    let entry_count = endian.u16(count_slice) as usize;
    if entry_count == 0 || entry_count > MAX_DCP_IFD_ENTRIES {
        return Err(CoreError::Validation(format!(
            ".dcp IFD entry count {entry_count} out of range 1..={MAX_DCP_IFD_ENTRIES}"
        )));
    }
    let entries_bytes = entry_count
        .checked_mul(12)
        .ok_or_else(|| CoreError::Validation(".dcp IFD size overflow".to_owned()))?;
    let entries_end = entries_start
        .checked_add(entries_bytes)
        .ok_or_else(|| CoreError::Validation(".dcp IFD end overflow".to_owned()))?;
    if entries_end > bytes.len() {
        return Err(CoreError::Validation(
            ".dcp IFD directory extends past end of file".to_owned(),
        ));
    }

    const MAX_DCP_HSV_FLOATS: usize = MAX_DCP_HUE_DIVS * MAX_DCP_SAT_DIVS * MAX_DCP_VAL_DIVS * 3;
    let mut profile = DcpProfile::default();
    let mut hsm_dims: Option<[usize; 3]> = None;
    let mut hsm_data_1: Option<Vec<f32>> = None;
    let mut hsm_data_2: Option<Vec<f32>> = None;
    let mut look_dims: Option<[usize; 3]> = None;
    let mut look_data: Option<Vec<f32>> = None;
    let mut seen_tag_mask: u16 = 0;

    for i in 0..entry_count {
        let off = entries_start + i * 12;
        let entry = &bytes[off..off + 12];
        let tag = endian.u16(&entry[0..2]);
        let typ = endian.u16(&entry[2..4]);
        let count = endian.u32(&entry[4..8]) as usize;
        let val_inline = &entry[8..12];

        let elem_size: usize = match typ {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 => 4,
            5 | 10 | 12 => 8,
            _ => continue,
        };
        let byte_len = count
            .checked_mul(elem_size)
            .ok_or_else(|| CoreError::Validation(format!(".dcp tag {tag} size overflow")))?;
        let data_slice: &[u8] = if byte_len <= 4 {
            &val_inline[..byte_len]
        } else {
            let data_off = endian.u32(val_inline) as usize;
            let data_end = data_off
                .checked_add(byte_len)
                .ok_or_else(|| CoreError::Validation(format!(".dcp tag {tag} offset overflow")))?;
            bytes.get(data_off..data_end).ok_or_else(|| {
                CoreError::Validation(format!(".dcp tag {tag} payload extends past EOF"))
            })?
        };

        let tag_bit: Option<u16> = match tag {
            50708 => Some(1 << 0),
            50936 => Some(1 << 1),
            50778 => Some(1 << 2),
            50779 => Some(1 << 3),
            50721 => Some(1 << 4),
            50722 => Some(1 << 5),
            50964 => Some(1 << 6),
            50965 => Some(1 << 7),
            50937 => Some(1 << 8),
            50938 => Some(1 << 9),
            50939 => Some(1 << 10),
            50981 => Some(1 << 11),
            50982 => Some(1 << 12),
            50940 => Some(1 << 13),
            _ => None,
        };
        if let Some(bit) = tag_bit {
            if (seen_tag_mask & bit) != 0 {
                return Err(CoreError::Validation(format!(
                    "Duplicate .dcp IFD tag {tag}"
                )));
            }
            seen_tag_mask |= bit;
        }

        match tag {
            // UniqueCameraModel (ASCII)
            50708 => {
                if let Some(s) = decode_tiff_ascii(data_slice) {
                    profile.unique_camera_model = Some(s);
                }
            }
            // ProfileName (ASCII)
            50936 => {
                if let Some(s) = decode_tiff_ascii(data_slice) {
                    profile.profile_name = Some(s);
                }
            }
            // CalibrationIlluminant1 / 2 (SHORT)
            50778 => {
                if count >= 1 && typ == 3 {
                    profile.calibration_illuminant_1 = Some(endian.u16(&data_slice[0..2]));
                }
            }
            50779 => {
                if count >= 1 && typ == 3 {
                    profile.calibration_illuminant_2 = Some(endian.u16(&data_slice[0..2]));
                }
            }
            // ColorMatrix1 / 2 & ForwardMatrix1 / 2
            50721 => {
                profile.color_matrix_1 =
                    Some(decode_tiff_mat9(data_slice, typ, count, endian, tag)?);
            }
            50722 => {
                profile.color_matrix_2 =
                    Some(decode_tiff_mat9(data_slice, typ, count, endian, tag)?);
            }
            50964 => {
                profile.forward_matrix_1 =
                    Some(decode_tiff_mat9(data_slice, typ, count, endian, tag)?);
            }
            50965 => {
                profile.forward_matrix_2 =
                    Some(decode_tiff_mat9(data_slice, typ, count, endian, tag)?);
            }
            // ProfileHueSatMapDims
            50937 => {
                hsm_dims = Some(decode_dcp_hsv_dims(data_slice, typ, count, endian, tag)?);
            }
            // ProfileHueSatMapData1 / 2
            50938 => {
                if count > MAX_DCP_HSV_FLOATS {
                    return Err(CoreError::Validation(format!(
                        ".dcp tag {tag} float count {count} exceeds cap {MAX_DCP_HSV_FLOATS}"
                    )));
                }
                hsm_data_1 = Some(decode_tiff_floats(data_slice, typ, count, endian, tag)?);
            }
            50939 => {
                if count > MAX_DCP_HSV_FLOATS {
                    return Err(CoreError::Validation(format!(
                        ".dcp tag {tag} float count {count} exceeds cap {MAX_DCP_HSV_FLOATS}"
                    )));
                }
                hsm_data_2 = Some(decode_tiff_floats(data_slice, typ, count, endian, tag)?);
            }
            // ProfileLookTableDims / Data
            50981 => {
                look_dims = Some(decode_dcp_hsv_dims(data_slice, typ, count, endian, tag)?);
            }
            50982 => {
                if count > MAX_DCP_HSV_FLOATS {
                    return Err(CoreError::Validation(format!(
                        ".dcp tag {tag} float count {count} exceeds cap {MAX_DCP_HSV_FLOATS}"
                    )));
                }
                look_data = Some(decode_tiff_floats(data_slice, typ, count, endian, tag)?);
            }
            // ProfileToneCurve
            50940 => {
                if count < 4 || !count.is_multiple_of(2) || count / 2 > MAX_DCP_TONE_CURVE_POINTS {
                    return Err(CoreError::Validation(format!(
                        ".dcp ProfileToneCurve float count {count} is invalid"
                    )));
                }
                let raw = decode_tiff_floats(data_slice, typ, count, endian, tag)?;
                let mut pts = Vec::with_capacity(raw.len() / 2);
                for pair in raw.as_chunks::<2>().0 {
                    pts.push([pair[0].clamp(0.0, 1.0), pair[1].clamp(0.0, 1.0)]);
                }
                pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
                profile.tone_curve = Some(pts);
            }
            _ => {}
        }
    }

    if seen_tag_mask == 0 {
        return Err(CoreError::Validation(
            ".dcp file contains no recognized DNG camera profile tags".to_owned(),
        ));
    }

    if let Some(raw) = hsm_data_1 {
        let dims = hsm_dims.ok_or_else(|| {
            CoreError::Validation(
                ".dcp has ProfileHueSatMapData1 without ProfileHueSatMapDims".to_owned(),
            )
        })?;
        profile.hue_sat_map_1 = Some(build_dcp_hsv_table(dims, &raw, "ProfileHueSatMapData1")?);
    }
    if let Some(raw) = hsm_data_2 {
        let dims = hsm_dims.ok_or_else(|| {
            CoreError::Validation(
                ".dcp has ProfileHueSatMapData2 without ProfileHueSatMapDims".to_owned(),
            )
        })?;
        profile.hue_sat_map_2 = Some(build_dcp_hsv_table(dims, &raw, "ProfileHueSatMapData2")?);
    }
    if let Some(raw) = look_data {
        let dims = look_dims.ok_or_else(|| {
            CoreError::Validation(
                ".dcp has ProfileLookTableData without ProfileLookTableDims".to_owned(),
            )
        })?;
        profile.look_table = Some(build_dcp_hsv_table(dims, &raw, "ProfileLookTableData")?);
    }

    Ok(profile)
}

fn decode_tiff_ascii(data: &[u8]) -> Option<String> {
    let trimmed = match data.iter().position(|&b| b == 0) {
        Some(pos) => &data[..pos],
        None => data,
    };
    let s = std::str::from_utf8(trimmed).ok()?.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

fn decode_tiff_floats(
    data: &[u8],
    typ: u16,
    count: usize,
    endian: Endian,
    tag: u16,
) -> Result<Vec<f32>, CoreError> {
    let mut out = Vec::with_capacity(count);
    match typ {
        11 => {
            for i in 0..count {
                let v = endian.f32(&data[i * 4..i * 4 + 4]);
                if !v.is_finite() {
                    return Err(CoreError::Validation(format!(
                        ".dcp tag {tag} contains non-finite float"
                    )));
                }
                out.push(v);
            }
        }
        12 => {
            for i in 0..count {
                let v = endian.f64(&data[i * 8..i * 8 + 8]) as f32;
                if !v.is_finite() {
                    return Err(CoreError::Validation(format!(
                        ".dcp tag {tag} contains non-finite double"
                    )));
                }
                out.push(v);
            }
        }
        10 => {
            for i in 0..count {
                let num = endian.i32(&data[i * 8..i * 8 + 4]) as f64;
                let den = endian.i32(&data[i * 8 + 4..i * 8 + 8]) as f64;
                if den == 0.0 {
                    return Err(CoreError::Validation(format!(
                        ".dcp tag {tag} contains zero SRATIONAL denominator"
                    )));
                }
                let v = (num / den) as f32;
                if !v.is_finite() {
                    return Err(CoreError::Validation(format!(
                        ".dcp tag {tag} contains non-finite SRATIONAL"
                    )));
                }
                out.push(v);
            }
        }
        5 => {
            for i in 0..count {
                let num = endian.u32(&data[i * 8..i * 8 + 4]) as f64;
                let den = endian.u32(&data[i * 8 + 4..i * 8 + 8]) as f64;
                if den == 0.0 {
                    return Err(CoreError::Validation(format!(
                        ".dcp tag {tag} contains zero RATIONAL denominator"
                    )));
                }
                let v = (num / den) as f32;
                if !v.is_finite() {
                    return Err(CoreError::Validation(format!(
                        ".dcp tag {tag} contains non-finite RATIONAL"
                    )));
                }
                out.push(v);
            }
        }
        _ => {
            return Err(CoreError::Validation(format!(
                ".dcp tag {tag} has unsupported numeric TIFF type {typ}"
            )));
        }
    }
    Ok(out)
}

fn decode_tiff_mat9(
    data: &[u8],
    typ: u16,
    count: usize,
    endian: Endian,
    tag: u16,
) -> Result<[f32; 9], CoreError> {
    if count != 9 {
        return Err(CoreError::Validation(format!(
            ".dcp matrix tag {tag} expected 9 elements, got {count}"
        )));
    }
    let vals = decode_tiff_floats(data, typ, count, endian, tag)?;
    let mut m = [0.0_f32; 9];
    m.copy_from_slice(&vals);
    Ok(m)
}

fn decode_dcp_hsv_dims(
    data: &[u8],
    typ: u16,
    count: usize,
    endian: Endian,
    tag: u16,
) -> Result<[usize; 3], CoreError> {
    if count != 3 {
        return Err(CoreError::Validation(format!(
            ".dcp dims tag {tag} expected 3 values, got {count}"
        )));
    }
    let vals: [usize; 3] = match typ {
        4 => [
            endian.u32(&data[0..4]) as usize,
            endian.u32(&data[4..8]) as usize,
            endian.u32(&data[8..12]) as usize,
        ],
        3 => [
            endian.u16(&data[0..2]) as usize,
            endian.u16(&data[2..4]) as usize,
            endian.u16(&data[4..6]) as usize,
        ],
        _ => {
            return Err(CoreError::Validation(format!(
                ".dcp dims tag {tag} expected LONG/SHORT type, got {typ}"
            )));
        }
    };
    let [h, s, v] = vals;
    if !(1..=MAX_DCP_HUE_DIVS).contains(&h)
        || !(1..=MAX_DCP_SAT_DIVS).contains(&s)
        || !(1..=MAX_DCP_VAL_DIVS).contains(&v)
    {
        return Err(CoreError::Validation(format!(
            ".dcp dims tag {tag} [{h}, {s}, {v}] exceeds safe caps [1..={MAX_DCP_HUE_DIVS}, 1..={MAX_DCP_SAT_DIVS}, 1..={MAX_DCP_VAL_DIVS}]"
        )));
    }
    Ok(vals)
}

fn build_dcp_hsv_table(
    dims: [usize; 3],
    raw: &[f32],
    label: &str,
) -> Result<DcpHsvTable, CoreError> {
    let [h, s, v] = dims;
    let total_entries = h
        .checked_mul(s)
        .and_then(|x| x.checked_mul(v))
        .ok_or_else(|| CoreError::Validation(format!(".dcp {label} dimension overflow")))?;
    let expected_floats = total_entries
        .checked_mul(3)
        .ok_or_else(|| CoreError::Validation(format!(".dcp {label} float count overflow")))?;
    if raw.len() != expected_floats {
        return Err(CoreError::Validation(format!(
            ".dcp {label} length mismatch: expected {expected_floats}, got {}",
            raw.len()
        )));
    }
    let mut data = Vec::with_capacity(total_entries);
    for chunk in raw.as_chunks::<3>().0 {
        data.push([
            chunk[0].clamp(-360.0, 360.0),
            chunk[1].clamp(0.0, 4.0),
            chunk[2].clamp(0.0, 4.0),
        ]);
    }
    Ok(DcpHsvTable {
        hue_divs: h,
        sat_divs: s,
        val_divs: v,
        data,
    })
}

impl DcpHsvTable {
    /// Applies this DNG HSV adjustment table (`hue_shift_deg`, `sat_scale`, `val_scale`)
    /// to a linear ProPhoto RGB pixel, preserving HDR headroom (`V > 1.0`) smoothly.
    #[inline]
    pub fn apply_linear(&self, rgb: [f32; 3]) -> [f32; 3] {
        let [h_deg, s, v] = rgb_to_hsv([rgb[0].max(0.0), rgb[1].max(0.0), rgb[2].max(0.0)]);
        if v <= 1e-7 {
            return [0.0, 0.0, 0.0];
        }

        // Periodic Hue axis [0, 360)
        let fh = (h_deg / 360.0) * (self.hue_divs as f32);
        let h0 = (fh.floor() as usize) % self.hue_divs;
        let h1 = (h0 + 1) % self.hue_divs;
        let th = if self.hue_divs > 1 {
            fh - fh.floor()
        } else {
            0.0
        };

        // Clamped Saturation axis [0, 1]
        let (s0, s1, ts) = if self.sat_divs > 1 {
            let fs = s.clamp(0.0, 1.0) * ((self.sat_divs - 1) as f32);
            let i0 = (fs.floor() as usize).min(self.sat_divs - 1);
            let i1 = (i0 + 1).min(self.sat_divs - 1);
            (i0, i1, fs - i0 as f32)
        } else {
            (0, 0, 0.0)
        };

        // Clamped Value axis [0, 1] (values > 1.0 sample top slice and scale proportionally)
        let (v0, v1, tv) = if self.val_divs > 1 {
            let fv = v.clamp(0.0, 1.0) * ((self.val_divs - 1) as f32);
            let i0 = (fv.floor() as usize).min(self.val_divs - 1);
            let i1 = (i0 + 1).min(self.val_divs - 1);
            (i0, i1, fv - i0 as f32)
        } else {
            (0, 0, 0.0)
        };

        let at = |vi: usize, hi: usize, si: usize| -> [f32; 3] {
            self.data[(vi * self.hue_divs + hi) * self.sat_divs + si]
        };

        let c000 = at(v0, h0, s0);
        let c001 = at(v0, h0, s1);
        let c010 = at(v0, h1, s0);
        let c011 = at(v0, h1, s1);
        let c100 = at(v1, h0, s0);
        let c101 = at(v1, h0, s1);
        let c110 = at(v1, h1, s0);
        let c111 = at(v1, h1, s1);

        let mut delta = [0.0_f32; 3];
        for ch in 0..3 {
            let a00 = c000[ch] + (c001[ch] - c000[ch]) * ts;
            let a01 = c010[ch] + (c011[ch] - c010[ch]) * ts;
            let a10 = c100[ch] + (c101[ch] - c100[ch]) * ts;
            let a11 = c110[ch] + (c111[ch] - c110[ch]) * ts;
            let b0 = a00 + (a01 - a00) * th;
            let b1 = a10 + (a11 - a10) * th;
            delta[ch] = b0 + (b1 - b0) * tv;
        }

        let h_new = (h_deg + delta[0]).rem_euclid(360.0);
        let s_new = (s * delta[1]).clamp(0.0, 1.0);
        let v_new = (v * delta[2]).max(0.0);
        hsv_to_rgb([h_new, s_new, v_new])
    }
}

/// Pre-baked 1D LUT representation of a DNG `ProfileToneCurve` on `[0, 1]` with C1 linear
/// HDR extension above `1.0`, constructed once per `bake_profile_lut` call (`O(1)` per texel).
struct PrebakedDcpToneCurve {
    lut: [f32; 1025],
    y1: f32,
    slope: f32,
}

impl PrebakedDcpToneCurve {
    fn from_points(pts: &[[f32; 2]]) -> Self {
        let spline = MonotonicSpline::from_points(pts);
        let mut lut = [0.0_f32; 1025];
        for (i, slot) in lut.iter_mut().enumerate() {
            *slot = spline.eval(i as f32 / 1024.0);
        }
        let y1 = lut[1024];
        let eps = 1e-3_f32;
        let y_prev = spline.eval(1.0 - eps);
        let slope = ((y1 - y_prev) / eps).clamp(0.1, 4.0);
        Self { lut, y1, slope }
    }

    #[inline]
    fn eval(&self, x: f32) -> f32 {
        let x = x.max(0.0);
        if x <= 1.0 {
            let f = x * 1024.0;
            let i0 = (f.floor() as usize).min(1024);
            let i1 = (i0 + 1).min(1024);
            let t = f - i0 as f32;
            self.lut[i0] + (self.lut[i1] - self.lut[i0]) * t
        } else {
            (self.y1 + self.slope * (x - 1.0)).max(0.0)
        }
    }
}

/// Adobe DNG Spec `RGBTone`: hue-preserving min/mid/max tone curve application.
#[inline]
fn apply_dcp_tone_curve_rgb(rgb: [f32; 3], curve: &PrebakedDcpToneCurve) -> [f32; 3] {
    let r = rgb[0].max(0.0);
    let g = rgb[1].max(0.0);
    let b = rgb[2].max(0.0);

    // Sort indices so c[i_min] <= c[i_mid] <= c[i_max].
    let mut idx = [0usize, 1, 2];
    let c = [r, g, b];
    if c[idx[0]] > c[idx[1]] {
        idx.swap(0, 1);
    }
    if c[idx[1]] > c[idx[2]] {
        idx.swap(1, 2);
    }
    if c[idx[0]] > c[idx[1]] {
        idx.swap(0, 1);
    }

    let c_min = c[idx[0]];
    let c_mid = c[idx[1]];
    let c_max = c[idx[2]];

    let y_min = curve.eval(c_min);
    let y_max = curve.eval(c_max);
    let y_mid = if c_max - c_min > 1e-6 {
        y_min + (y_max - y_min) * ((c_mid - c_min) / (c_max - c_min))
    } else {
        y_min
    };

    let mut out = [0.0_f32; 3];
    out[idx[0]] = y_min.max(0.0);
    out[idx[1]] = y_mid.max(0.0);
    out[idx[2]] = y_max.max(0.0);
    out
}

impl DcpProfile {
    /// Selects the daylight/D65 `HueSatMap` table when dual-illuminant tables are present.
    pub fn primary_hue_sat_map(&self) -> Option<&DcpHsvTable> {
        const D65_TAG: u16 = 21;
        match (&self.hue_sat_map_1, &self.hue_sat_map_2) {
            (Some(t1), Some(t2)) => {
                if self.calibration_illuminant_2 == Some(D65_TAG) {
                    Some(t2)
                } else if self.calibration_illuminant_1 == Some(D65_TAG) {
                    Some(t1)
                } else {
                    Some(t2)
                }
            }
            (Some(t1), None) => Some(t1),
            (None, Some(t2)) => Some(t2),
            (None, None) => None,
        }
    }

    /// Evaluates this `.dcp` profile (`HueSatMap` -> `LookTable` -> hue-preserving `ProfileToneCurve`)
    /// in Bradford-adapted ProPhoto RGB working space and bakes cleanly into our `33³` [`ProfileLut3D`].
    pub fn bake_profile_lut(&self) -> ProfileLut3D {
        let hsm = self.primary_hue_sat_map();
        let look = self.look_table.as_ref();
        let curve = self
            .tone_curve
            .as_deref()
            .map(PrebakedDcpToneCurve::from_points);

        if hsm.is_none() && look.is_none() && curve.is_none() {
            return ProfileLut3D::identity(PROFILE_LUT_SIZE);
        }

        ProfileLut3D::from_linear_fn(PROFILE_LUT_SIZE, |lin_srgb| {
            let mut pro = mul_mat3_vec3(&SRGB_D65_TO_PROPHOTO_D50, lin_srgb);
            pro = [pro[0].max(0.0), pro[1].max(0.0), pro[2].max(0.0)];

            if let Some(table) = hsm {
                pro = table.apply_linear(pro);
            }
            if let Some(table) = look {
                pro = table.apply_linear(pro);
            }
            if let Some(ref c) = curve {
                pro = apply_dcp_tone_curve_rgb(pro, c);
            }

            let out = mul_mat3_vec3(&PROPHOTO_D50_TO_SRGB_D65, pro);
            [out[0].max(0.0), out[1].max(0.0), out[2].max(0.0)]
        })
    }
}

// ── 3. Pure-Rust `.icc` / `.icm` Camera Input Profile Parser ─────────────────

/// ICC Tone Reproduction Curve (`curv` or `para` types `0..=4`).
#[derive(Clone, Debug, PartialEq)]
pub enum IccTrc {
    Identity,
    Gamma(f32),
    Table(Vec<f32>),
    Parametric { func_type: u16, params: [f32; 7] },
}

impl IccTrc {
    #[inline]
    pub fn evaluate(&self, x: f32) -> f32 {
        let x = if x.is_finite() {
            x.clamp(0.0, 1.0)
        } else {
            0.0
        };
        match self {
            Self::Identity => x,
            Self::Gamma(g) => x.powf((*g).clamp(0.05, 16.0)),
            Self::Table(tbl) => sample_1d_table(tbl, x),
            Self::Parametric { func_type, params } => {
                let [g, a, b, c, d, e, f] = *params;
                let g = g.clamp(0.05, 16.0);
                let out = match *func_type {
                    0 => x.powf(g),
                    1 => {
                        if a.abs() > 1e-6 && x >= -b / a {
                            (a * x + b).max(0.0).powf(g)
                        } else {
                            0.0
                        }
                    }
                    2 => {
                        if a.abs() > 1e-6 && x >= -b / a {
                            (a * x + b).max(0.0).powf(g) + c
                        } else {
                            c
                        }
                    }
                    3 => {
                        if x >= d {
                            (a * x + b).max(0.0).powf(g)
                        } else {
                            c * x
                        }
                    }
                    4 => {
                        if x >= d {
                            (a * x + b).max(0.0).powf(g) + e
                        } else {
                            c * x + f
                        }
                    }
                    _ => x,
                };
                if out.is_finite() { out.max(0.0) } else { 0.0 }
            }
        }
    }
}

/// Parsed ICC `A2B0` 3D CLUT (`mft1` 8-bit or `mft2` 16-bit).
#[derive(Clone, Debug, PartialEq)]
pub struct IccClut3D {
    pub grid_size: usize,
    pub is_16bit: bool,
    pub input_tables: [Vec<f32>; 3],
    /// R-outermost / B-fastest per ISO 15076-1 §10.8–§10.9 (`((r * G + g) * G + b)`).
    pub clut: Vec<[f32; 3]>,
    pub output_tables: [Vec<f32>; 3],
}

/// ICC Profile Connection Space (`XYZ ` or `Lab `).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IccPcs {
    XyzD50,
    LabD50,
}

/// Parsed ICC / ICM color profile (`matrix-shaper` and/or `A2B0` 3D CLUT).
#[derive(Clone, Debug, PartialEq)]
pub struct IccProfile {
    pub description: Option<String>,
    pub pcs: IccPcs,
    pub matrix_xyz_d50: Option<[f32; 9]>,
    pub trcs: Option<[IccTrc; 3]>,
    pub a2b0: Option<IccClut3D>,
}

/// Parses an ICC v2 / v4 RGB input/working profile (`.icc` / `.icm`) with strict header,
/// tag-table, curve, and CLUT dimension bounds checks.
pub fn parse_icc_profile(bytes: &[u8]) -> Result<IccProfile, CoreError> {
    if bytes.len() < 132 {
        return Err(CoreError::Validation(
            ".icc file is truncated (smaller than 128-byte header + tag count)".to_owned(),
        ));
    }
    if bytes.len() > MAX_PROFILE_FILE_BYTES {
        return Err(CoreError::Validation(format!(
            ".icc file exceeds {} MiB cap",
            MAX_PROFILE_FILE_BYTES / (1024 * 1024)
        )));
    }
    let profile_size = be_u32(&bytes[0..4]) as usize;
    if profile_size < 132 || profile_size > bytes.len() {
        return Err(CoreError::Validation(format!(
            "Invalid .icc profile_size {profile_size} (file len {})",
            bytes.len()
        )));
    }
    if &bytes[36..40] != b"acsp" {
        return Err(CoreError::Validation(
            "Invalid .icc signature (expected 'acsp' at offset 36)".to_owned(),
        ));
    }
    if &bytes[16..20] != b"RGB " {
        return Err(CoreError::Validation(
            "Only RGB .icc/.icm profiles are supported".to_owned(),
        ));
    }
    let pcs = match &bytes[20..24] {
        b"XYZ " => IccPcs::XyzD50,
        b"Lab " => IccPcs::LabD50,
        other => {
            return Err(CoreError::Validation(format!(
                "Unsupported .icc PCS '{:?}' (expected 'XYZ ' or 'Lab ')",
                String::from_utf8_lossy(other)
            )));
        }
    };

    let tag_count = be_u32(&bytes[128..132]) as usize;
    if tag_count == 0 || tag_count > MAX_ICC_TAG_COUNT {
        return Err(CoreError::Validation(format!(
            ".icc tag_count {tag_count} out of supported range 1..={MAX_ICC_TAG_COUNT}"
        )));
    }
    let table_end = 132usize
        .checked_add(
            tag_count
                .checked_mul(12)
                .ok_or_else(|| CoreError::Validation(".icc tag table overflow".to_owned()))?,
        )
        .ok_or_else(|| CoreError::Validation(".icc tag table end overflow".to_owned()))?;
    if table_end > profile_size {
        return Err(CoreError::Validation(
            ".icc tag table extends past profile_size".to_owned(),
        ));
    }

    let mut description: Option<String> = None;
    let mut r_xyz: Option<[f32; 3]> = None;
    let mut g_xyz: Option<[f32; 3]> = None;
    let mut b_xyz: Option<[f32; 3]> = None;
    let mut r_trc: Option<IccTrc> = None;
    let mut g_trc: Option<IccTrc> = None;
    let mut b_trc: Option<IccTrc> = None;
    let mut a2b0: Option<IccClut3D> = None;
    let mut seen_icc_mask: u8 = 0;

    for i in 0..tag_count {
        let rec = &bytes[132 + i * 12..132 + (i + 1) * 12];
        let sig = &rec[0..4];
        let off = be_u32(&rec[4..8]) as usize;
        let len = be_u32(&rec[8..12]) as usize;
        let end = off
            .checked_add(len)
            .ok_or_else(|| CoreError::Validation(".icc tag offset overflow".to_owned()))?;
        let payload = bytes
            .get(off..end)
            .filter(|_| end <= profile_size)
            .ok_or_else(|| CoreError::Validation(".icc tag payload out of bounds".to_owned()))?;

        let sig_bit: Option<u8> = match sig {
            b"desc" => Some(1 << 0),
            b"rXYZ" => Some(1 << 1),
            b"gXYZ" => Some(1 << 2),
            b"bXYZ" => Some(1 << 3),
            b"rTRC" => Some(1 << 4),
            b"gTRC" => Some(1 << 5),
            b"bTRC" => Some(1 << 6),
            b"A2B0" => Some(1 << 7),
            _ => None,
        };
        if let Some(bit) = sig_bit {
            if (seen_icc_mask & bit) != 0 {
                return Err(CoreError::Validation(format!(
                    "Duplicate .icc tag signature '{}'",
                    String::from_utf8_lossy(sig)
                )));
            }
            seen_icc_mask |= bit;
        }

        match sig {
            b"desc" => {
                description = parse_icc_desc(payload);
            }
            b"rXYZ" => r_xyz = Some(parse_icc_xyz(payload)?),
            b"gXYZ" => g_xyz = Some(parse_icc_xyz(payload)?),
            b"bXYZ" => b_xyz = Some(parse_icc_xyz(payload)?),
            b"rTRC" => r_trc = Some(parse_icc_trc(payload)?),
            b"gTRC" => g_trc = Some(parse_icc_trc(payload)?),
            b"bTRC" => b_trc = Some(parse_icc_trc(payload)?),
            b"A2B0"
                if payload.len() >= 4
                    && (&payload[0..4] == b"mft1" || &payload[0..4] == b"mft2") =>
            {
                a2b0 = Some(parse_icc_mft(payload)?);
            }
            _ => {}
        }
    }

    let matrix_xyz_d50 = match (r_xyz, g_xyz, b_xyz) {
        (Some(r), Some(g), Some(b)) => Some([
            r[0], g[0], b[0], //
            r[1], g[1], b[1], //
            r[2], g[2], b[2],
        ]),
        _ => None,
    };
    let trcs = match (r_trc, g_trc, b_trc) {
        (Some(r), Some(g), Some(b)) => Some([r, g, b]),
        _ => None,
    };

    if a2b0.is_none() && (matrix_xyz_d50.is_none() || trcs.is_none()) {
        return Err(CoreError::Validation(
            ".icc profile has neither an A2B0 mft1/mft2 LUT nor complete RGB matrix-shaper tags"
                .to_owned(),
        ));
    }

    Ok(IccProfile {
        description,
        pcs,
        matrix_xyz_d50,
        trcs,
        a2b0,
    })
}

#[inline]
fn be_u16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

#[inline]
fn be_u32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

#[inline]
fn be_s15fixed16(b: &[u8]) -> f32 {
    i32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f32 / 65536.0
}

fn parse_icc_desc(payload: &[u8]) -> Option<String> {
    if payload.len() < 12 {
        return None;
    }
    match &payload[0..4] {
        // ICC v2 textDescriptionType
        b"desc" => {
            let count = be_u32(&payload[8..12]) as usize;
            let end = 12usize.checked_add(count)?;
            let raw = payload.get(12..end)?;
            decode_tiff_ascii(raw)
        }
        // ICC v4 multiLocalizedUnicodeType
        b"mluc" => {
            if payload.len() < 28 {
                return None;
            }
            let rec_count = be_u32(&payload[8..12]) as usize;
            let rec_size = be_u32(&payload[12..16]) as usize;
            if rec_count == 0 || rec_size < 12 {
                return None;
            }
            let str_len = be_u32(&payload[20..24]) as usize;
            let str_off = be_u32(&payload[24..28]) as usize;
            let end = str_off.checked_add(str_len)?;
            let raw = payload.get(str_off..end)?;
            if raw.len() < 2 || !raw.len().is_multiple_of(2) {
                return None;
            }
            let u16s: Vec<u16> = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes(*c))
                .collect();
            let s = String::from_utf16_lossy(&u16s);
            let trimmed = s.trim_matches('\0').trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        }
        _ => None,
    }
}

fn parse_icc_xyz(payload: &[u8]) -> Result<[f32; 3], CoreError> {
    if payload.len() < 20 || &payload[0..4] != b"XYZ " {
        return Err(CoreError::Validation(
            "Invalid .icc XYZ tag payload".to_owned(),
        ));
    }
    let xyz = [
        be_s15fixed16(&payload[8..12]),
        be_s15fixed16(&payload[12..16]),
        be_s15fixed16(&payload[16..20]),
    ];
    if !xyz.iter().all(|v| v.is_finite()) {
        return Err(CoreError::Validation(
            "Non-finite value in .icc XYZ tag".to_owned(),
        ));
    }
    Ok(xyz)
}

fn parse_icc_trc(payload: &[u8]) -> Result<IccTrc, CoreError> {
    if payload.len() < 12 {
        return Err(CoreError::Validation(
            "Truncated .icc TRC tag payload".to_owned(),
        ));
    }
    match &payload[0..4] {
        b"curv" => {
            let count = be_u32(&payload[8..12]) as usize;
            if count > MAX_ICC_CURVE_ENTRIES {
                return Err(CoreError::Validation(format!(
                    ".icc curv entry count {count} exceeds cap {MAX_ICC_CURVE_ENTRIES}"
                )));
            }
            let needed = 12usize
                .checked_add(
                    count
                        .checked_mul(2)
                        .ok_or_else(|| CoreError::Validation(".icc curv overflow".to_owned()))?,
                )
                .ok_or_else(|| CoreError::Validation(".icc curv end overflow".to_owned()))?;
            if payload.len() < needed {
                return Err(CoreError::Validation(
                    "Truncated .icc curv table".to_owned(),
                ));
            }
            match count {
                0 => Ok(IccTrc::Identity),
                1 => {
                    let gamma = be_u16(&payload[12..14]) as f32 / 256.0;
                    if !gamma.is_finite() || gamma <= 0.0 {
                        return Err(CoreError::Validation("Invalid .icc curv gamma".to_owned()));
                    }
                    Ok(IccTrc::Gamma(gamma))
                }
                _ => {
                    let mut tbl = Vec::with_capacity(count);
                    for i in 0..count {
                        tbl.push(be_u16(&payload[12 + i * 2..14 + i * 2]) as f32 / 65535.0);
                    }
                    Ok(IccTrc::Table(tbl))
                }
            }
        }
        b"para" => {
            let func_type = be_u16(&payload[8..10]);
            let param_count: usize = match func_type {
                0 => 1,
                1 => 3,
                2 => 4,
                3 => 5,
                4 => 7,
                _ => {
                    return Err(CoreError::Validation(format!(
                        "Unsupported .icc para function type {func_type} (expected 0..=4)"
                    )));
                }
            };
            let needed = 12 + param_count * 4;
            if payload.len() < needed {
                return Err(CoreError::Validation(
                    "Truncated .icc para parameters".to_owned(),
                ));
            }
            let mut params = [0.0_f32; 7];
            for (i, slot) in params.iter_mut().take(param_count).enumerate() {
                let v = be_s15fixed16(&payload[12 + i * 4..16 + i * 4]);
                if !v.is_finite() {
                    return Err(CoreError::Validation(
                        "Non-finite .icc para parameter".to_owned(),
                    ));
                }
                *slot = v;
            }
            Ok(IccTrc::Parametric { func_type, params })
        }
        other => Err(CoreError::Validation(format!(
            "Unsupported .icc TRC tag type '{}'",
            String::from_utf8_lossy(other)
        ))),
    }
}

fn parse_icc_mft(payload: &[u8]) -> Result<IccClut3D, CoreError> {
    let is_16bit = match &payload[0..4] {
        b"mft1" => false,
        b"mft2" => true,
        _ => {
            return Err(CoreError::Validation(
                "Expected mft1 or mft2 tag signature".to_owned(),
            ));
        }
    };
    let header_len = if is_16bit { 52 } else { 48 };
    if payload.len() < header_len {
        return Err(CoreError::Validation(
            "Truncated .icc mft1/mft2 header".to_owned(),
        ));
    }
    let in_ch = payload[8] as usize;
    let out_ch = payload[9] as usize;
    let grid = payload[10] as usize;
    if in_ch != 3 || out_ch != 3 {
        return Err(CoreError::Validation(format!(
            "Unsupported .icc mft channel count {in_ch}->{out_ch} (expected 3->3)"
        )));
    }
    if !(2..=MAX_ICC_CLUT_GRID).contains(&grid) {
        return Err(CoreError::Validation(format!(
            ".icc CLUT grid size {grid} out of supported range 2..={MAX_ICC_CLUT_GRID}"
        )));
    }

    let (in_entries, out_entries) = if is_16bit {
        let i_n = be_u16(&payload[48..50]) as usize;
        let o_n = be_u16(&payload[50..52]) as usize;
        if !(2..=4096).contains(&i_n) || !(2..=4096).contains(&o_n) {
            return Err(CoreError::Validation(format!(
                ".icc mft2 table entries ({i_n}, {o_n}) out of range 2..=4096"
            )));
        }
        (i_n, o_n)
    } else {
        (256usize, 256usize)
    };

    let sample_bytes: usize = if is_16bit { 2 } else { 1 };
    let in_bytes = in_entries
        .checked_mul(3)
        .and_then(|v| v.checked_mul(sample_bytes))
        .ok_or_else(|| CoreError::Validation(".icc mft input table overflow".to_owned()))?;
    let clut_points = grid
        .checked_mul(grid)
        .and_then(|v| v.checked_mul(grid))
        .ok_or_else(|| CoreError::Validation(".icc CLUT grid overflow".to_owned()))?;
    let clut_bytes = clut_points
        .checked_mul(3)
        .and_then(|v| v.checked_mul(sample_bytes))
        .ok_or_else(|| CoreError::Validation(".icc CLUT byte size overflow".to_owned()))?;
    let out_bytes = out_entries
        .checked_mul(3)
        .and_then(|v| v.checked_mul(sample_bytes))
        .ok_or_else(|| CoreError::Validation(".icc mft output table overflow".to_owned()))?;

    let clut_off = header_len
        .checked_add(in_bytes)
        .ok_or_else(|| CoreError::Validation(".icc mft offset overflow".to_owned()))?;
    let out_off = clut_off
        .checked_add(clut_bytes)
        .ok_or_else(|| CoreError::Validation(".icc mft offset overflow".to_owned()))?;
    let total_end = out_off
        .checked_add(out_bytes)
        .ok_or_else(|| CoreError::Validation(".icc mft size overflow".to_owned()))?;
    if total_end > payload.len() {
        return Err(CoreError::Validation(
            "Truncated .icc mft1/mft2 payload".to_owned(),
        ));
    }

    let read_norm = |slice: &[u8], idx: usize| -> f32 {
        if is_16bit {
            be_u16(&slice[idx * 2..idx * 2 + 2]) as f32 / 65535.0
        } else {
            slice[idx] as f32 / 255.0
        }
    };

    let in_slice = &payload[header_len..clut_off];
    let mut input_tables = [
        Vec::with_capacity(in_entries),
        Vec::with_capacity(in_entries),
        Vec::with_capacity(in_entries),
    ];
    for (ch, tbl) in input_tables.iter_mut().enumerate() {
        let base = ch * in_entries;
        for i in 0..in_entries {
            tbl.push(read_norm(in_slice, base + i));
        }
    }

    let clut_slice = &payload[clut_off..out_off];
    let mut clut = Vec::with_capacity(clut_points);
    for i in 0..clut_points {
        clut.push([
            read_norm(clut_slice, i * 3),
            read_norm(clut_slice, i * 3 + 1),
            read_norm(clut_slice, i * 3 + 2),
        ]);
    }

    let out_slice = &payload[out_off..total_end];
    let mut output_tables = [
        Vec::with_capacity(out_entries),
        Vec::with_capacity(out_entries),
        Vec::with_capacity(out_entries),
    ];
    for (ch, tbl) in output_tables.iter_mut().enumerate() {
        let base = ch * out_entries;
        for i in 0..out_entries {
            tbl.push(read_norm(out_slice, base + i));
        }
    }

    Ok(IccClut3D {
        grid_size: grid,
        is_16bit,
        input_tables,
        clut,
        output_tables,
    })
}

#[inline]
fn sample_1d_table(tbl: &[f32], x: f32) -> f32 {
    if tbl.is_empty() {
        return x;
    }
    if tbl.len() == 1 {
        return tbl[0];
    }
    let f = x.clamp(0.0, 1.0) * (tbl.len() - 1) as f32;
    let i0 = (f.floor() as usize).min(tbl.len() - 1);
    let i1 = (i0 + 1).min(tbl.len() - 1);
    let t = f - i0 as f32;
    tbl[i0] + (tbl[i1] - tbl[i0]) * t
}

impl IccClut3D {
    /// Evaluates the ICC `mft1`/`mft2` pipeline (`input_tables` -> R-outermost/B-fastest 3D CLUT -> `output_tables`).
    #[inline]
    pub fn evaluate_norm(&self, rgb: [f32; 3]) -> [f32; 3] {
        let r_in = sample_1d_table(&self.input_tables[0], rgb[0]);
        let g_in = sample_1d_table(&self.input_tables[1], rgb[1]);
        let b_in = sample_1d_table(&self.input_tables[2], rgb[2]);

        let g = self.grid_size;
        let max_i = (g - 1) as f32;
        let fr = r_in.clamp(0.0, 1.0) * max_i;
        let fg = g_in.clamp(0.0, 1.0) * max_i;
        let fb = b_in.clamp(0.0, 1.0) * max_i;

        let r0 = (fr.floor() as usize).min(g - 1);
        let g0 = (fg.floor() as usize).min(g - 1);
        let b0 = (fb.floor() as usize).min(g - 1);
        let r1 = (r0 + 1).min(g - 1);
        let g1 = (g0 + 1).min(g - 1);
        let b1 = (b0 + 1).min(g - 1);

        let tr = fr - r0 as f32;
        let tg = fg - g0 as f32;
        let tb = fb - b0 as f32;

        // ISO 15076-1 §10.8-10.9: R varies least rapidly, B varies most rapidly.
        let at =
            |ri: usize, gi: usize, bi: usize| -> [f32; 3] { self.clut[(ri * g + gi) * g + bi] };

        let c000 = at(r0, g0, b0);
        let c001 = at(r0, g0, b1);
        let c010 = at(r0, g1, b0);
        let c011 = at(r0, g1, b1);
        let c100 = at(r1, g0, b0);
        let c101 = at(r1, g0, b1);
        let c110 = at(r1, g1, b0);
        let c111 = at(r1, g1, b1);

        let mut mid = [0.0_f32; 3];
        for ch in 0..3 {
            let a00 = c000[ch] + (c001[ch] - c000[ch]) * tb;
            let a01 = c010[ch] + (c011[ch] - c010[ch]) * tb;
            let a10 = c100[ch] + (c101[ch] - c100[ch]) * tb;
            let a11 = c110[ch] + (c111[ch] - c110[ch]) * tb;
            let d0 = a00 + (a01 - a00) * tg;
            let d1 = a10 + (a11 - a10) * tg;
            mid[ch] = d0 + (d1 - d0) * tr;
        }

        [
            sample_1d_table(&self.output_tables[0], mid[0]),
            sample_1d_table(&self.output_tables[1], mid[1]),
            sample_1d_table(&self.output_tables[2], mid[2]),
        ]
    }
}

#[inline]
fn cie_lab_d50_to_xyz_d50(lab: [f32; 3]) -> [f32; 3] {
    let fy = (lab[0] + 16.0) / 116.0;
    let fx = fy + lab[1] / 500.0;
    let fz = fy - lab[2] / 200.0;

    let inv_f = |t: f32| -> f32 {
        const DELTA: f32 = 6.0 / 29.0;
        if t > DELTA {
            t * t * t
        } else {
            3.0 * DELTA * DELTA * (t - 4.0 / 29.0)
        }
    };
    [
        D50_XYZ[0] * inv_f(fx),
        D50_XYZ[1] * inv_f(fy),
        D50_XYZ[2] * inv_f(fz),
    ]
}

impl IccProfile {
    /// Bakes this ICC profile (preferring `A2B0` `mft1`/`mft2` when present, else matrix-shaper
    /// `rXYZ`/`gXYZ`/`bXYZ` + `rTRC`/`gTRC`/`bTRC`) into our unified `33³` linear [`ProfileLut3D`],
    /// adapting PCS D50 -> Bradford D65 -> Linear sRGB cleanly.
    pub fn bake_profile_lut(&self) -> ProfileLut3D {
        let pcs_to_srgb = xyz_d50_to_linear_srgb_matrix();

        if let Some(a2b0) = &self.a2b0 {
            let pcs = self.pcs;
            // u1Fixed15Number scale: 0x8000 (32768) = 1.0 in mft2, 0x80 (128) = 1.0 in mft1.
            let xyz_scale = if a2b0.is_16bit {
                65535.0_f32 / 32768.0_f32
            } else {
                255.0_f32 / 128.0_f32
            };
            return ProfileLut3D::from_linear_fn(PROFILE_LUT_SIZE, |lin| {
                let sdr = [
                    lin[0].clamp(0.0, 1.0),
                    lin[1].clamp(0.0, 1.0),
                    lin[2].clamp(0.0, 1.0),
                ];
                let hdr_excess = [
                    (lin[0] - 1.0).max(0.0),
                    (lin[1] - 1.0).max(0.0),
                    (lin[2] - 1.0).max(0.0),
                ];
                let pcs_norm = a2b0.evaluate_norm(sdr);
                let xyz_d50 = match pcs {
                    IccPcs::XyzD50 => [
                        pcs_norm[0] * xyz_scale,
                        pcs_norm[1] * xyz_scale,
                        pcs_norm[2] * xyz_scale,
                    ],
                    IccPcs::LabD50 => {
                        // ISO 15076-1 §10.9 Table 41:
                        // - 16-bit mft2 uses legacy ICC v2 CIELAB encoding:
                        //   L* = 0xFF00 -> 100.0, a*/b* = 0x8000 -> 0.0, 0xFF00 -> +127.0
                        // - 8-bit mft1 maps 0xFF -> 100.0, 0x80 -> 0.0, 0xFF -> +127.0
                        let (l_star, a_star, b_star) = if a2b0.is_16bit {
                            (
                                pcs_norm[0] * (65535.0_f32 / 65280.0_f32) * 100.0,
                                pcs_norm[1] * (65535.0_f32 / 256.0_f32) - 128.0,
                                pcs_norm[2] * (65535.0_f32 / 256.0_f32) - 128.0,
                            )
                        } else {
                            (
                                pcs_norm[0] * 100.0,
                                pcs_norm[1] * 255.0 - 128.0,
                                pcs_norm[2] * 255.0 - 128.0,
                            )
                        };
                        cie_lab_d50_to_xyz_d50([l_star, a_star, b_star])
                    }
                };
                let srgb = mul_mat3_vec3(&pcs_to_srgb, xyz_d50);
                [
                    (srgb[0].max(0.0) + hdr_excess[0]).max(0.0),
                    (srgb[1].max(0.0) + hdr_excess[1]).max(0.0),
                    (srgb[2].max(0.0) + hdr_excess[2]).max(0.0),
                ]
            });
        }

        if let (Some(mat_d50), Some(trcs)) = (self.matrix_xyz_d50, &self.trcs) {
            let rgb_to_srgb = mul_mat3(&pcs_to_srgb, &mat_d50);
            return ProfileLut3D::from_linear_fn(PROFILE_LUT_SIZE, |lin| {
                let sdr = [
                    lin[0].clamp(0.0, 1.0),
                    lin[1].clamp(0.0, 1.0),
                    lin[2].clamp(0.0, 1.0),
                ];
                let hdr_excess = [
                    (lin[0] - 1.0).max(0.0),
                    (lin[1] - 1.0).max(0.0),
                    (lin[2] - 1.0).max(0.0),
                ];
                let shaped = [
                    trcs[0].evaluate(sdr[0]) + hdr_excess[0],
                    trcs[1].evaluate(sdr[1]) + hdr_excess[1],
                    trcs[2].evaluate(sdr[2]) + hdr_excess[2],
                ];
                let out = mul_mat3_vec3(&rgb_to_srgb, shaped);
                [out[0].max(0.0), out[1].max(0.0), out[2].max(0.0)]
            });
        }

        ProfileLut3D::identity(PROFILE_LUT_SIZE)
    }
}

// ── 4. Safe User Profile Store & Cache ───────────────────────────────────────

/// Parsed external profile metadata (validated without baking a `33³` LUT).
#[derive(Clone, Debug)]
struct ParsedExternalMeta {
    display_name: Option<String>,
    camera_match: Option<String>,
    format_tag: &'static str,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct UserProfileSidecarMeta {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    camera_match: Option<String>,
    #[serde(default)]
    description: String,
}

fn parse_external_profile_meta_only(
    bytes: &[u8],
    ext_lower: &str,
) -> Result<ParsedExternalMeta, CoreError> {
    match ext_lower {
        "cube" => {
            let cube = parse_cube_lut(bytes)?;
            Ok(ParsedExternalMeta {
                display_name: cube.title,
                camera_match: None,
                format_tag: "CUBE",
            })
        }
        "dcp" => {
            let dcp = parse_dcp_profile(bytes)?;
            Ok(ParsedExternalMeta {
                display_name: dcp.profile_name,
                camera_match: dcp.unique_camera_model,
                format_tag: "DCP",
            })
        }
        "icc" | "icm" => {
            let icc = parse_icc_profile(bytes)?;
            Ok(ParsedExternalMeta {
                display_name: icc.description.clone(),
                camera_match: icc.description,
                format_tag: "ICC",
            })
        }
        other => Err(CoreError::Validation(format!(
            "Unsupported profile extension '.{other}' (allowed: .dcp, .cube, .icc, .icm)"
        ))),
    }
}

fn parse_and_bake_external_profile_lut(
    bytes: &[u8],
    ext_lower: &str,
) -> Result<ProfileLut3D, CoreError> {
    match ext_lower {
        "cube" => Ok(parse_cube_lut(bytes)?.bake_profile_lut()),
        "dcp" => Ok(parse_dcp_profile(bytes)?.bake_profile_lut()),
        "icc" | "icm" => Ok(parse_icc_profile(bytes)?.bake_profile_lut()),
        other => Err(CoreError::Validation(format!(
            "Unsupported profile extension '.{other}' (allowed: .dcp, .cube, .icc, .icm)"
        ))),
    }
}

/// Returns the default user camera profiles directory (`~/.config/rune-darkroom/profiles`).
pub fn default_profiles_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("rune-darkroom").join("profiles");
    }
    if let Some(home) = std::env::var_os("HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home)
            .join(".config")
            .join("rune-darkroom")
            .join("profiles");
    }
    PathBuf::from(".rune-darkroom-profiles")
}

/// Validates and converts a user profile name into a safe `[a-z0-9_-]` filename slug.
///
/// Strictly rejects empty strings, `/`, `\`, `..`, leading `.`, and NUL bytes to guarantee
/// zero path traversal under the user profiles directory.
pub fn sanitize_profile_slug(name: &str) -> Result<String, CoreError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(CoreError::Validation(
            "Profile name cannot be empty".to_owned(),
        ));
    }
    if trimmed.contains("..")
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains('\0')
        || trimmed.starts_with('.')
    {
        return Err(CoreError::Validation(format!(
            "Invalid profile name '{name}': path separators, leading dots, and '..' are forbidden"
        )));
    }

    let mut slug = String::with_capacity(trimmed.len().min(64));
    let mut prev_sep = false;
    for ch in trimmed.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            slug.push(lower);
            prev_sep = false;
        } else if (lower == '-' || lower == '_' || lower.is_ascii_whitespace())
            && !prev_sep
            && !slug.is_empty()
        {
            slug.push(if lower == '-' { '-' } else { '_' });
            prev_sep = true;
        }
        if slug.len() >= 64 {
            break;
        }
    }
    while slug.ends_with('_') || slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        return Err(CoreError::Validation(format!(
            "Profile name '{name}' contains no valid alphanumeric characters"
        )));
    }
    Ok(slug)
}

fn read_bounded_profile_file(path: &Path) -> Result<(Vec<u8>, String), CoreError> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(CoreError::Validation(format!(
            "Refusing symlink profile path '{}'",
            path.display()
        )));
    }
    if !meta.file_type().is_file() {
        return Err(CoreError::Validation(format!(
            "Profile path '{}' is not a regular file",
            path.display()
        )));
    }
    if meta.len() > MAX_PROFILE_FILE_BYTES as u64 {
        return Err(CoreError::Validation(format!(
            "Profile file '{}' exceeds {} MiB limit",
            path.display(),
            MAX_PROFILE_FILE_BYTES / (1024 * 1024)
        )));
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .ok_or_else(|| {
            CoreError::Validation(format!(
                "Profile '{}' is missing a valid extension (.dcp, .cube, .icc, .icm)",
                path.display()
            ))
        })?;
    if !ALLOWED_PROFILE_EXTENSIONS.contains(&ext.as_str()) {
        return Err(CoreError::Validation(format!(
            "Disallowed profile extension '.{ext}' (allowed: .dcp, .cube, .icc, .icm)"
        )));
    }

    let file = File::open(path)?;
    let mut bytes = Vec::with_capacity((meta.len() as usize).min(MAX_PROFILE_FILE_BYTES));
    file.take(MAX_PROFILE_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_PROFILE_FILE_BYTES {
        return Err(CoreError::Validation(format!(
            "Profile file '{}' grew beyond {} MiB limit during read",
            path.display(),
            MAX_PROFILE_FILE_BYTES / (1024 * 1024)
        )));
    }
    Ok((bytes, ext))
}

fn find_user_profile_file(dir: &Path, slug: &str) -> Option<(PathBuf, String)> {
    for &ext in ALLOWED_PROFILE_EXTENSIONS {
        let candidate = dir.join(format!("{slug}.{ext}"));
        if candidate.parent() != Some(dir) {
            continue;
        }
        if let Ok(meta) = std::fs::symlink_metadata(&candidate)
            && meta.file_type().is_file()
            && !meta.file_type().is_symlink()
        {
            return Some((candidate, ext.to_owned()));
        }
    }
    None
}

/// Imports an external `.dcp`, `.cube`, `.icc`, or `.icm` profile into `~/.config/rune-darkroom/profiles`,
/// rejecting symlinks, oversized files, invalid payloads, and accidental overwrites.
pub fn import_user_profile(
    src: &Path,
    name_override: Option<&str>,
) -> Result<CameraProfileDescriptor, CoreError> {
    import_user_profile_in_dir(&default_profiles_dir(), src, name_override)
}

/// Hermetic variant of [`import_user_profile`] storing validated profiles inside `dir`
/// (refusing to overwrite an existing profile with the same slug).
pub fn import_user_profile_in_dir(
    dir: &Path,
    src: &Path,
    name_override: Option<&str>,
) -> Result<CameraProfileDescriptor, CoreError> {
    import_user_profile_in_dir_with_overwrite(dir, src, name_override, false)
}

/// Imports an external camera profile into `dir` with explicit overwrite control.
pub fn import_user_profile_in_dir_with_overwrite(
    dir: &Path,
    src: &Path,
    name_override: Option<&str>,
    overwrite: bool,
) -> Result<CameraProfileDescriptor, CoreError> {
    let (bytes, ext) = read_bounded_profile_file(src)?;
    // Strictly validate by parsing BEFORE touching the store directory (no wasted 33³ bake).
    let parsed = parse_external_profile_meta_only(&bytes, &ext)?;

    let stem_fallback = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("profile");
    let chosen_name = name_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or(parsed.display_name.as_deref())
        .unwrap_or(stem_fallback)
        .to_owned();
    let slug = sanitize_profile_slug(&chosen_name)?;

    std::fs::create_dir_all(dir)?;
    let dest = dir.join(format!("{slug}.{ext}"));
    if dest.parent() != Some(dir) {
        return Err(CoreError::Validation(format!(
            "Refusing profile path outside store directory: '{slug}'"
        )));
    }
    let existing = find_user_profile_file(dir, &slug);
    let dest_meta = std::fs::symlink_metadata(&dest).ok();

    if let Some(ref m) = dest_meta
        && (m.file_type().is_symlink() || !m.file_type().is_file())
    {
        return Err(CoreError::Validation(format!(
            "Refusing to overwrite symlink or non-regular file at '{}'",
            dest.display()
        )));
    }
    if !overwrite {
        if let Some((ref existing_path, _)) = existing {
            return Err(CoreError::Validation(format!(
                "User profile 'user:{slug}' already exists at '{}' (pass a different name or overwrite=true)",
                existing_path.display()
            )));
        }
        if dest_meta.is_some() {
            return Err(CoreError::Validation(format!(
                "Destination path '{}' already exists (pass a different name or overwrite=true)",
                dest.display()
            )));
        }
    }

    // Write the new profile atomically FIRST so an I/O error never destroys an existing profile.
    atomic_write_bytes(&dest, &bytes)?;

    if let Some((existing_path, _)) = existing
        && existing_path != dest
    {
        let _ = std::fs::remove_file(&existing_path);
    }

    let description = format!(
        "Imported {} camera profile ({})",
        parsed.format_tag,
        dest.file_name().and_then(|s| s.to_str()).unwrap_or("")
    );
    let sidecar_meta = UserProfileSidecarMeta {
        name: chosen_name.clone(),
        camera_match: parsed.camera_match.clone(),
        description: description.clone(),
    };
    let meta_path = dir.join(format!("{slug}.meta.json"));
    if let Ok(meta_json) = serde_json::to_vec_pretty(&sidecar_meta) {
        let _ = atomic_write_bytes(&meta_path, &meta_json);
    }

    invalidate_cached_user_profile(dir, &slug);

    Ok(CameraProfileDescriptor {
        id: format!("{USER_PROFILE_PREFIX}{slug}"),
        name: chosen_name,
        category: ProfileCategory::User,
        description,
        camera_match: parsed.camera_match,
    })
}

/// Lists all valid user profiles stored in `~/.config/rune-darkroom/profiles`.
pub fn list_user_profiles() -> Result<Vec<CameraProfileDescriptor>, CoreError> {
    list_user_profiles_in_dir(&default_profiles_dir())
}

/// Lists all valid user profiles stored in `dir`, skipping symlinks, hidden dotfiles,
/// disallowed extensions, and corrupt/oversized files.
pub fn list_user_profiles_in_dir(dir: &Path) -> Result<Vec<CameraProfileDescriptor>, CoreError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(fname) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if fname.starts_with('.') {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.file_type().is_file() {
            continue;
        }
        let Some(ext) = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
        else {
            continue;
        };
        if !ALLOWED_PROFILE_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(slug) = sanitize_profile_slug(stem) else {
            continue;
        };
        if slug != stem {
            continue;
        }
        let Ok((bytes, ext_l)) = read_bounded_profile_file(&path) else {
            continue;
        };
        let Ok(parsed) = parse_external_profile_meta_only(&bytes, &ext_l) else {
            continue;
        };

        let meta_path = dir.join(format!("{slug}.meta.json"));
        let sidecar = std::fs::symlink_metadata(&meta_path)
            .ok()
            .filter(|m| {
                m.file_type().is_file() && !m.file_type().is_symlink() && m.len() <= 64 * 1024
            })
            .and_then(|_| std::fs::read(&meta_path).ok())
            .and_then(|b| serde_json::from_slice::<UserProfileSidecarMeta>(&b).ok());

        let name = sidecar
            .as_ref()
            .map(|m| m.name.clone())
            .filter(|n| !n.trim().is_empty())
            .or(parsed.display_name)
            .unwrap_or_else(|| slug.clone());
        let camera_match = sidecar
            .as_ref()
            .and_then(|m| m.camera_match.clone())
            .or(parsed.camera_match);
        let description = sidecar
            .as_ref()
            .map(|m| m.description.clone())
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| format!("Imported {} camera profile ({fname})", parsed.format_tag));

        out.push(CameraProfileDescriptor {
            id: format!("{USER_PROFILE_PREFIX}{slug}"),
            name,
            category: ProfileCategory::User,
            description,
            camera_match,
        });
    }
    out.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(out)
}

/// Deletes a user profile by slug (or `"user:<slug>"`) from `~/.config/rune-darkroom/profiles`.
pub fn delete_user_profile(slug: &str) -> Result<bool, CoreError> {
    delete_user_profile_in_dir(&default_profiles_dir(), slug)
}

/// Deletes a user profile by slug (or `"user:<slug>"`) from `dir` and invalidates its LUT cache.
pub fn delete_user_profile_in_dir(dir: &Path, slug: &str) -> Result<bool, CoreError> {
    let raw = slug.trim();
    let bare = raw.strip_prefix(USER_PROFILE_PREFIX).unwrap_or(raw);
    let clean_slug = sanitize_profile_slug(bare)?;

    let mut removed = false;
    for &ext in ALLOWED_PROFILE_EXTENSIONS {
        let p = dir.join(format!("{clean_slug}.{ext}"));
        if p.parent() == Some(dir)
            && let Ok(meta) = std::fs::symlink_metadata(&p)
            && meta.file_type().is_file()
            && !meta.file_type().is_symlink()
        {
            std::fs::remove_file(&p)?;
            removed = true;
        }
    }
    let meta_path = dir.join(format!("{clean_slug}.meta.json"));
    if meta_path.parent() == Some(dir)
        && let Ok(meta) = std::fs::symlink_metadata(&meta_path)
        && meta.file_type().is_file()
        && !meta.file_type().is_symlink()
    {
        let _ = std::fs::remove_file(&meta_path);
    }
    invalidate_cached_user_profile(dir, &clean_slug);
    Ok(removed)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    mtime: Option<SystemTime>,
    inode: Option<u64>,
}

#[derive(Clone)]
struct CachedUserLut {
    stamp: FileStamp,
    lut: Option<Arc<ProfileLut3D>>,
}

fn user_lut_cache() -> &'static Mutex<HashMap<PathBuf, CachedUserLut>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedUserLut>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn file_stamp(meta: &std::fs::Metadata) -> FileStamp {
    #[cfg(unix)]
    let inode = {
        use std::os::unix::fs::MetadataExt;
        Some(meta.ino())
    };
    #[cfg(not(unix))]
    let inode = None;
    FileStamp {
        len: meta.len(),
        mtime: meta.modified().ok(),
        inode,
    }
}

fn invalidate_cached_user_profile(dir: &Path, slug: &str) {
    if let Ok(mut guard) = user_lut_cache().lock() {
        for &ext in ALLOWED_PROFILE_EXTENSIONS {
            let p = dir.join(format!("{slug}.{ext}"));
            guard.remove(&p);
        }
    }
}

/// Resolves any camera profile ID (bundled MIT profile OR `"user:<slug>"` external profile)
/// to a cached `Arc<ProfileLut3D>`.
///
/// Returns `None` for `"auto"`, `"linear_scientific"`, missing user profiles, or invalid IDs
/// so GPU/CPU pipelines take their zero-cost identity fast path.
/// Automatically invalidates cached user LUTs whenever file length, mtime, or inode changes.
pub fn resolve_profile_lut(
    profile_id: &str,
    profiles_dir: Option<&Path>,
) -> Option<Arc<ProfileLut3D>> {
    let trimmed = profile_id.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(lut) = cached_builtin_profile_lut(trimmed) {
        return Some(lut);
    }

    let rest = trimmed.strip_prefix(USER_PROFILE_PREFIX)?;
    let slug = sanitize_profile_slug(rest).ok()?;
    let owned_default;
    let dir = match profiles_dir {
        Some(d) => d,
        None => {
            owned_default = default_profiles_dir();
            owned_default.as_path()
        }
    };

    let (path, ext) = find_user_profile_file(dir, &slug)?;
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if meta.file_type().is_symlink() || !meta.file_type().is_file() {
        return None;
    }
    let stamp = file_stamp(&meta);

    if let Ok(guard) = user_lut_cache().lock()
        && let Some(hit) = guard.get(&path)
        && hit.stamp == stamp
    {
        return hit.lut.as_ref().map(Arc::clone);
    }

    let (bytes, _) = read_bounded_profile_file(&path).ok()?;
    let baked = parse_and_bake_external_profile_lut(&bytes, &ext).ok()?;
    let lut = if baked.is_identity() {
        None
    } else {
        Some(Arc::new(baked))
    };

    if let Ok(mut guard) = user_lut_cache().lock() {
        if guard.len() >= MAX_CACHED_USER_LUTS && !guard.contains_key(&path) {
            guard.clear();
        }
        guard.insert(
            path,
            CachedUserLut {
                stamp,
                lut: lut.as_ref().map(Arc::clone),
            },
        );
    }
    lut
}

fn tokenize_camera_words(raw: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            cur.push(ch.to_ascii_lowercase());
        } else if !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

fn token_alnum_len(tokens: &[String]) -> usize {
    tokens.iter().map(String::len).sum()
}

fn contains_token_subsequence(haystack: &[String], needle: &[String]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn camera_profile_matches_exif(
    candidate: Option<&str>,
    camera_make: Option<&str>,
    camera_model: Option<&str>,
) -> bool {
    let Some(cand_raw) = candidate
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "*")
    else {
        return false;
    };
    let cand = tokenize_camera_words(cand_raw);
    if cand.is_empty() || token_alnum_len(&cand) < 3 {
        return false;
    }

    let make = camera_make.map(tokenize_camera_words).unwrap_or_default();
    let model_raw = camera_model.map(tokenize_camera_words).unwrap_or_default();
    if model_raw.is_empty() {
        return false;
    }

    // Deduplicate leading make tokens when EXIF model already includes the make (e.g. "Canon EOS R5").
    let model_stripped: &[String] = if !make.is_empty()
        && model_raw.len() > make.len()
        && model_raw[..make.len()] == make[..]
    {
        &model_raw[make.len()..]
    } else {
        &model_raw[..]
    };

    let mut full = Vec::with_capacity(make.len() + model_stripped.len());
    full.extend_from_slice(&make);
    full.extend_from_slice(model_stripped);

    let matches_target = |target: &[String]| -> bool {
        if target.is_empty() {
            return false;
        }
        let len = token_alnum_len(target);
        if cand == target && len >= 3 {
            return true;
        }
        len >= 4 && contains_token_subsequence(&cand, target)
    };

    matches_target(&full) || matches_target(&model_raw) || matches_target(model_stripped)
}

/// Lists all available camera profiles (user profiles + 14 bundled MIT profiles),
/// auto-matching user `.dcp`/`.icc` profiles against EXIF `(camera_make, camera_model)`
/// and sorting camera-matched user profiles to the top!
pub fn list_all_camera_profiles(
    camera_make: Option<&str>,
    camera_model: Option<&str>,
    profiles_dir: Option<&Path>,
) -> Vec<CameraProfileDescriptor> {
    let owned_default;
    let dir = match profiles_dir {
        Some(d) => d,
        None => {
            owned_default = default_profiles_dir();
            owned_default.as_path()
        }
    };
    let user_profiles = list_user_profiles_in_dir(dir).unwrap_or_default();

    let mut matched_user = Vec::new();
    let mut other_user = Vec::new();
    for mut desc in user_profiles {
        if camera_profile_matches_exif(desc.camera_match.as_deref(), camera_make, camera_model) {
            if desc.camera_match.is_none() {
                desc.camera_match = camera_model.or(camera_make).map(str::to_owned);
            }
            matched_user.push(desc);
        } else {
            other_user.push(desc);
        }
    }

    let builtins = builtin_camera_profiles();
    let mut out = Vec::with_capacity(matched_user.len() + builtins.len() + other_user.len());
    out.extend(matched_user);
    out.extend(builtins);
    out.extend(other_user);
    out
}

#[cfg(test)]
pub(crate) mod test_fixtures {
    /// Builds a valid synthetic binary Adobe `.dcp` profile (`II` or `MM`, magic `0x4352`)
    /// with `UniqueCameraModel`, `ProfileName`, `CalibrationIlluminant1`, `ProfileHueSatMapDims/Data1`,
    /// `ProfileLookTableDims/Data`, and `ProfileToneCurve`.
    pub fn build_synthetic_dcp(
        big_endian: bool,
        camera_model: &str,
        profile_name: &str,
    ) -> Vec<u8> {
        let mut buf = Vec::new();
        let push_u16 = |b: &mut Vec<u8>, v: u16| {
            if big_endian {
                b.extend_from_slice(&v.to_be_bytes());
            } else {
                b.extend_from_slice(&v.to_le_bytes());
            }
        };
        let push_u32 = |b: &mut Vec<u8>, v: u32| {
            if big_endian {
                b.extend_from_slice(&v.to_be_bytes());
            } else {
                b.extend_from_slice(&v.to_le_bytes());
            }
        };
        let push_f32 = |b: &mut Vec<u8>, v: f32| {
            if big_endian {
                b.extend_from_slice(&v.to_be_bytes());
            } else {
                b.extend_from_slice(&v.to_le_bytes());
            }
        };

        // Header
        buf.extend_from_slice(if big_endian { b"MM" } else { b"II" });
        push_u16(&mut buf, 0x4352);
        push_u32(&mut buf, 8); // IFD0 offset

        let mut model_bytes = camera_model.as_bytes().to_vec();
        model_bytes.push(0);
        let mut name_bytes = profile_name.as_bytes().to_vec();
        name_bytes.push(0);

        // HueSatMap: 6 hue x 2 sat x 2 val (subtle warm red shift + slight sat boost)
        let hsm_dims = [6u32, 2, 2];
        let mut hsm_data = Vec::new();
        for _v in 0..2 {
            for h in 0..6 {
                for s in 0..2 {
                    let hue_shift = if h == 0 && s == 1 { 8.0_f32 } else { 0.0_f32 };
                    let sat_scale = if s == 1 { 1.12_f32 } else { 1.0_f32 };
                    let val_scale = 1.0_f32;
                    hsm_data.extend_from_slice(&[hue_shift, sat_scale, val_scale]);
                }
            }
        }

        // LookTable: 2 hue x 2 sat x 2 val (gentle highlight lift)
        let look_dims = [2u32, 2, 2];
        let mut look_data = Vec::new();
        for v in 0..2 {
            for _h in 0..2 {
                for _s in 0..2 {
                    let val_scale = if v == 1 { 1.04_f32 } else { 0.98_f32 };
                    look_data.extend_from_slice(&[0.0_f32, 1.05_f32, val_scale]);
                }
            }
        }

        // Tone curve: gentle filmic S-curve
        let curve_floats: [f32; 8] = [0.0, 0.0, 0.25, 0.22, 0.75, 0.80, 1.0, 1.0];

        // 8 IFD entries (sorted by tag ID):
        // 50708 UniqueCameraModel, 50778 CalibrationIlluminant1, 50936 ProfileName,
        // 50937 HueSatMapDims, 50938 HueSatMapData1, 50940 ProfileToneCurve,
        // 50981 LookTableDims, 50982 LookTableData
        let entry_count = 8u16;
        let data_start = 8u32 + 2 + (entry_count as u32) * 12 + 4;

        let mut payload = Vec::new();
        let append_bytes = |p: &mut Vec<u8>, src: &[u8]| -> u32 {
            let off = data_start + p.len() as u32;
            p.extend_from_slice(src);
            while !p.len().is_multiple_of(4) {
                p.push(0);
            }
            off
        };

        let model_off = append_bytes(&mut payload, &model_bytes);
        let name_off = append_bytes(&mut payload, &name_bytes);

        let mut dims_b = Vec::new();
        for d in hsm_dims {
            push_u32(&mut dims_b, d);
        }
        let hsm_dims_off = append_bytes(&mut payload, &dims_b);

        let mut hsm_b = Vec::new();
        for f in &hsm_data {
            push_f32(&mut hsm_b, *f);
        }
        let hsm_data_off = append_bytes(&mut payload, &hsm_b);

        let mut curve_b = Vec::new();
        for f in curve_floats {
            push_f32(&mut curve_b, f);
        }
        let curve_off = append_bytes(&mut payload, &curve_b);

        let mut ldims_b = Vec::new();
        for d in look_dims {
            push_u32(&mut ldims_b, d);
        }
        let look_dims_off = append_bytes(&mut payload, &ldims_b);

        let mut look_b = Vec::new();
        for f in &look_data {
            push_f32(&mut look_b, *f);
        }
        let look_data_off = append_bytes(&mut payload, &look_b);

        push_u16(&mut buf, entry_count);
        let push_entry = |b: &mut Vec<u8>, tag: u16, typ: u16, count: u32, val_off: u32| {
            push_u16(b, tag);
            push_u16(b, typ);
            push_u32(b, count);
            push_u32(b, val_off);
        };
        let push_short_inline = |b: &mut Vec<u8>, tag: u16, val: u16| {
            push_u16(b, tag);
            push_u16(b, 3);
            push_u32(b, 1);
            push_u16(b, val);
            push_u16(b, 0);
        };

        push_entry(&mut buf, 50708, 2, model_bytes.len() as u32, model_off);
        push_short_inline(&mut buf, 50778, 21); // D65
        push_entry(&mut buf, 50936, 2, name_bytes.len() as u32, name_off);
        push_entry(&mut buf, 50937, 4, 3, hsm_dims_off);
        push_entry(&mut buf, 50938, 11, hsm_data.len() as u32, hsm_data_off);
        push_entry(&mut buf, 50940, 11, curve_floats.len() as u32, curve_off);
        push_entry(&mut buf, 50981, 4, 3, look_dims_off);
        push_entry(&mut buf, 50982, 11, look_data.len() as u32, look_data_off);
        push_u32(&mut buf, 0); // next IFD
        assert_eq!(buf.len() as u32, data_start);
        buf.extend_from_slice(&payload);
        buf
    }

    /// Builds a valid synthetic `.cube` (`17^3`) with non-default `DOMAIN_MIN`/`DOMAIN_MAX`,
    /// comments, and a warm cinematic color grade.
    pub fn build_synthetic_cube_17(title: &str, warm_r: f32, cool_b: f32) -> Vec<u8> {
        let mut s = String::new();
        s.push_str("# Synthetic Rune Darkroom test .cube LUT\n");
        s.push_str(&format!("TITLE \"{title}\"\n"));
        s.push_str("LUT_3D_SIZE 17\n");
        s.push_str("DOMAIN_MIN 0.02 0.02 0.02\n");
        s.push_str("DOMAIN_MAX 0.98 0.98 0.98\n\n");
        let n = 17usize;
        for b in 0..n {
            let fb = b as f32 / (n - 1) as f32;
            for g in 0..n {
                let fg = g as f32 / (n - 1) as f32;
                for r in 0..n {
                    let fr = r as f32 / (n - 1) as f32;
                    let or = (fr.powf(warm_r)).clamp(0.0, 1.0);
                    let og = fg.clamp(0.0, 1.0);
                    let ob = (fb.powf(cool_b)).clamp(0.0, 1.0);
                    s.push_str(&format!("{or:.6} {og:.6} {ob:.6}\n"));
                }
            }
        }
        s.into_bytes()
    }

    /// Builds a valid synthetic ICC v2 matrix-shaper profile (`rXYZ`/`gXYZ`/`bXYZ` + `curv` or `para`).
    pub fn build_synthetic_icc_matrix_shaper(
        desc: &str,
        r_xyz: [f32; 3],
        g_xyz: [f32; 3],
        b_xyz: [f32; 3],
        use_para: bool,
    ) -> Vec<u8> {
        let make_xyz = |v: [f32; 3]| -> Vec<u8> {
            let mut p = b"XYZ \0\0\0\0".to_vec();
            for c in v {
                p.extend_from_slice(&((c * 65536.0).round() as i32).to_be_bytes());
            }
            p
        };
        let make_trc = || -> Vec<u8> {
            if use_para {
                // para type 0: gentle contrast gamma 1.15
                let mut p = b"para\0\0\0\0".to_vec();
                p.extend_from_slice(&0u16.to_be_bytes());
                p.extend_from_slice(&0u16.to_be_bytes());
                p.extend_from_slice(&((1.15_f32 * 65536.0).round() as i32).to_be_bytes());
                p
            } else {
                // curv count = 0 (exact linear identity)
                let mut p = b"curv\0\0\0\0".to_vec();
                p.extend_from_slice(&0u32.to_be_bytes());
                p
            }
        };
        let mut desc_p = b"desc\0\0\0\0".to_vec();
        let mut ascii = desc.as_bytes().to_vec();
        ascii.push(0);
        desc_p.extend_from_slice(&(ascii.len() as u32).to_be_bytes());
        desc_p.extend_from_slice(&ascii);

        let trc = make_trc();
        let tags: [([u8; 4], Vec<u8>); 7] = [
            (*b"desc", desc_p),
            (*b"rXYZ", make_xyz(r_xyz)),
            (*b"gXYZ", make_xyz(g_xyz)),
            (*b"bXYZ", make_xyz(b_xyz)),
            (*b"rTRC", trc.clone()),
            (*b"gTRC", trc.clone()),
            (*b"bTRC", trc),
        ];
        assemble_icc_container(*b"XYZ ", &tags)
    }

    /// Builds a valid synthetic ICC v2 `A2B0` `mft2` 16-bit 3D CLUT profile (`grid=9`).
    pub fn build_synthetic_icc_a2b0_mft2(desc: &str) -> Vec<u8> {
        let mut desc_p = b"desc\0\0\0\0".to_vec();
        let mut ascii = desc.as_bytes().to_vec();
        ascii.push(0);
        desc_p.extend_from_slice(&(ascii.len() as u32).to_be_bytes());
        desc_p.extend_from_slice(&ascii);

        let grid = 9usize;
        let in_n = 2usize;
        let out_n = 2usize;
        let mut mft = b"mft2\0\0\0\0".to_vec();
        mft.push(3); // in_ch
        mft.push(3); // out_ch
        mft.push(grid as u8);
        mft.push(0); // pad
        // Identity 3x3 matrix (ignored for RGB input per ICC spec)
        for v in [1.0_f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
            mft.extend_from_slice(&((v * 65536.0).round() as i32).to_be_bytes());
        }
        mft.extend_from_slice(&(in_n as u16).to_be_bytes());
        mft.extend_from_slice(&(out_n as u16).to_be_bytes());
        // 3 identity input tables
        for _ in 0..3 {
            mft.extend_from_slice(&0u16.to_be_bytes());
            mft.extend_from_slice(&65535u16.to_be_bytes());
        }
        // 3D CLUT (R-outermost, B-fastest) encoding sRGB D50 with slight warm lift in u1Fixed15Number
        let srgb_d50 = [
            0.436_075_f32,
            0.385_065,
            0.143_080,
            0.222_505,
            0.716_879,
            0.060_617,
            0.013_932,
            0.097_105,
            0.714_173,
        ];
        for r in 0..grid {
            let fr = (r as f32 / (grid - 1) as f32).powf(0.92);
            for g in 0..grid {
                let fg = g as f32 / (grid - 1) as f32;
                for b in 0..grid {
                    let fb = (b as f32 / (grid - 1) as f32).powf(1.06);
                    let x = srgb_d50[0] * fr + srgb_d50[1] * fg + srgb_d50[2] * fb;
                    let y = srgb_d50[3] * fr + srgb_d50[4] * fg + srgb_d50[5] * fb;
                    let z = srgb_d50[6] * fr + srgb_d50[7] * fg + srgb_d50[8] * fb;
                    for c in [x, y, z] {
                        let u = (c * 32768.0).round().clamp(0.0, 65535.0) as u16;
                        mft.extend_from_slice(&u.to_be_bytes());
                    }
                }
            }
        }
        // 3 identity output tables
        for _ in 0..3 {
            mft.extend_from_slice(&0u16.to_be_bytes());
            mft.extend_from_slice(&65535u16.to_be_bytes());
        }

        let tags: [([u8; 4], Vec<u8>); 2] = [(*b"desc", desc_p), (*b"A2B0", mft)];
        assemble_icc_container(*b"XYZ ", &tags)
    }

    fn assemble_icc_container(pcs: [u8; 4], tags: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut header = vec![0u8; 128];
        header[12..16].copy_from_slice(b"scnr");
        header[16..20].copy_from_slice(b"RGB ");
        header[20..24].copy_from_slice(&pcs);
        header[36..40].copy_from_slice(b"acsp");

        let mut table = Vec::new();
        table.extend_from_slice(&(tags.len() as u32).to_be_bytes());
        let data_start = 128 + 4 + tags.len() * 12;
        let mut payload = Vec::new();
        for (sig, data) in tags {
            let off = data_start + payload.len();
            table.extend_from_slice(sig);
            table.extend_from_slice(&(off as u32).to_be_bytes());
            table.extend_from_slice(&(data.len() as u32).to_be_bytes());
            payload.extend_from_slice(data);
            while !payload.len().is_multiple_of(4) {
                payload.push(0);
            }
        }

        let total = (header.len() + table.len() + payload.len()) as u32;
        header[0..4].copy_from_slice(&total.to_be_bytes());
        let mut out = Vec::with_capacity(total as usize);
        out.extend_from_slice(&header);
        out.extend_from_slice(&table);
        out.extend_from_slice(&payload);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::test_fixtures::*;
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_tmp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "rune_profile_io_{label}_{}_{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn cube_parser_resamples_non_default_domain_and_preserves_hdr() {
        let bytes = build_synthetic_cube_17("Warm Cinema 17", 0.85, 1.15);
        let cube = parse_cube_lut(&bytes).expect("parse 17^3 .cube");
        assert_eq!(cube.title.as_deref(), Some("Warm Cinema 17"));
        assert_eq!(cube.size, 17);
        assert_eq!(cube.domain_min, [0.02, 0.02, 0.02]);
        assert_eq!(cube.domain_max, [0.98, 0.98, 0.98]);

        let lut = cube.bake_profile_lut();
        assert_eq!(lut.size, PROFILE_LUT_SIZE);
        assert!(!lut.is_identity());

        // Midtone gray shifts warm (R > G > B).
        let mid = lut.sample_trilinear([0.18, 0.18, 0.18]);
        assert!(
            mid[0] > mid[1] + 0.02 && mid[1] > mid[2] + 0.02,
            "mid={mid:?}"
        );

        // HDR > 1.0 preserves headroom smoothly above 1.0 without hard clipping.
        let hdr = lut.sample_trilinear([2.5, 2.5, 2.5]);
        assert!(hdr[0] > 2.2 && hdr[1] > 2.2 && hdr[2] > 2.2, "hdr={hdr:?}");
    }

    #[test]
    fn dcp_parser_supports_le_and_be_and_bakes_cleanly() {
        for big_endian in [false, true] {
            let bytes = build_synthetic_dcp(big_endian, "SONY ILCE-7RM5", "Sony Camera Vivid");
            let dcp = parse_dcp_profile(&bytes).expect("parse synthetic .dcp");
            assert_eq!(dcp.unique_camera_model.as_deref(), Some("SONY ILCE-7RM5"));
            assert_eq!(dcp.profile_name.as_deref(), Some("Sony Camera Vivid"));
            assert!(dcp.hue_sat_map_1.is_some());
            assert!(dcp.look_table.is_some());
            assert!(dcp.tone_curve.is_some());

            let lut = dcp.bake_profile_lut();
            assert_eq!(lut.size, PROFILE_LUT_SIZE);
            assert!(!lut.is_identity());
            let red = lut.sample_trilinear([0.5, 0.1, 0.08]);
            assert!(red.iter().all(|v| v.is_finite() && *v >= 0.0));
            // Neutral gray remains strictly achromatic.
            let gray = lut.sample_trilinear([0.35, 0.35, 0.35]);
            let spread = gray[0].max(gray[1]).max(gray[2]) - gray[0].min(gray[1]).min(gray[2]);
            assert!(spread < 2e-3, "DCP neutral gray drifted: {gray:?}");
        }
    }

    #[test]
    fn icc_parser_supports_matrix_shaper_curv_para_and_a2b0_mft2() {
        // 1. Identity linear sRGB D50 matrix-shaper (curv=0) preserves RGB accurately.
        let srgb_icc = build_synthetic_icc_matrix_shaper(
            "Linear sRGB D50",
            [0.436_075, 0.222_505, 0.013_932],
            [0.385_065, 0.716_879, 0.097_105],
            [0.143_080, 0.060_617, 0.714_173],
            false,
        );
        let parsed = parse_icc_profile(&srgb_icc).expect("parse matrix-shaper .icc");
        assert_eq!(parsed.description.as_deref(), Some("Linear sRGB D50"));
        let lut = parsed.bake_profile_lut();
        for px in [[0.18, 0.18, 0.18], [0.55, 0.22, 0.12], [0.10, 0.30, 0.65]] {
            let out = lut.sample_trilinear(px);
            for ch in 0..3 {
                assert!(
                    (out[ch] - px[ch]).abs() < 2e-3,
                    "linear sRGB D50 ICC drift on {px:?} ch {ch}: got {}",
                    out[ch]
                );
            }
        }

        // 2. Parametric para TRC (gamma 1.15) darkens midtones while preserving neutral gray.
        let para_icc = build_synthetic_icc_matrix_shaper(
            "Contrast Camera ICC",
            [0.436_075, 0.222_505, 0.013_932],
            [0.385_065, 0.716_879, 0.097_105],
            [0.143_080, 0.060_617, 0.714_173],
            true,
        );
        let para_lut = parse_icc_profile(&para_icc).unwrap().bake_profile_lut();
        let mid = para_lut.sample_trilinear([0.25, 0.25, 0.25]);
        assert!(
            mid[0] < 0.23 && (mid[0] - mid[2]).abs() < 1e-3,
            "mid={mid:?}"
        );

        // 3. A2B0 mft2 16-bit 3D CLUT parses cleanly and reflects warm lift.
        let mft_icc = build_synthetic_icc_a2b0_mft2("Leica SL3 Warm mft2");
        let parsed_mft = parse_icc_profile(&mft_icc).expect("parse mft2 .icc");
        assert!(parsed_mft.a2b0.is_some());
        let mft_lut = parsed_mft.bake_profile_lut();
        let mft_mid = mft_lut.sample_trilinear([0.25, 0.25, 0.25]);
        assert!(
            mft_mid[0] > mft_mid[1] + 0.01 && mft_mid[1] > mft_mid[2] + 0.01,
            "mft2 warm grade expected R>G>B, got {mft_mid:?}"
        );

        // 4. ICC v4 `mluc` UTF-16BE description + `para` type 3 (sRGB piecewise) + `mft1` 8-bit CLUT.
        let mut mluc_p = b"mluc\0\0\0\0".to_vec();
        mluc_p.extend_from_slice(&1u32.to_be_bytes()); // 1 record
        mluc_p.extend_from_slice(&12u32.to_be_bytes()); // record size
        mluc_p.extend_from_slice(b"enUS");
        let utf16: Vec<u8> = "Fuji GFX v4"
            .encode_utf16()
            .flat_map(|u| u.to_be_bytes())
            .collect();
        mluc_p.extend_from_slice(&(utf16.len() as u32).to_be_bytes());
        mluc_p.extend_from_slice(&28u32.to_be_bytes()); // string offset
        mluc_p.extend_from_slice(&utf16);

        let mut para3 = b"para\0\0\0\0".to_vec();
        para3.extend_from_slice(&3u16.to_be_bytes());
        para3.extend_from_slice(&0u16.to_be_bytes());
        for v in [2.4_f32, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045] {
            para3.extend_from_slice(&((v * 65536.0).round() as i32).to_be_bytes());
        }
        let make_xyz = |v: [f32; 3]| -> Vec<u8> {
            let mut p = b"XYZ \0\0\0\0".to_vec();
            for c in v {
                p.extend_from_slice(&((c * 65536.0).round() as i32).to_be_bytes());
            }
            p
        };
        let mut v4_header = vec![0u8; 128];
        v4_header[12..16].copy_from_slice(b"scnr");
        v4_header[16..20].copy_from_slice(b"RGB ");
        v4_header[20..24].copy_from_slice(b"XYZ ");
        v4_header[36..40].copy_from_slice(b"acsp");
        let v4_tags: [([u8; 4], Vec<u8>); 7] = [
            (*b"desc", mluc_p),
            (*b"rXYZ", make_xyz([0.436_075, 0.222_505, 0.013_932])),
            (*b"gXYZ", make_xyz([0.385_065, 0.716_879, 0.097_105])),
            (*b"bXYZ", make_xyz([0.143_080, 0.060_617, 0.714_173])),
            (*b"rTRC", para3.clone()),
            (*b"gTRC", para3.clone()),
            (*b"bTRC", para3),
        ];
        let mut v4_tbl = Vec::new();
        v4_tbl.extend_from_slice(&(v4_tags.len() as u32).to_be_bytes());
        let v4_start = 128 + 4 + v4_tags.len() * 12;
        let mut v4_pay = Vec::new();
        for (sig, data) in &v4_tags {
            v4_tbl.extend_from_slice(sig);
            v4_tbl.extend_from_slice(&((v4_start + v4_pay.len()) as u32).to_be_bytes());
            v4_tbl.extend_from_slice(&(data.len() as u32).to_be_bytes());
            v4_pay.extend_from_slice(data);
            while !v4_pay.len().is_multiple_of(4) {
                v4_pay.push(0);
            }
        }
        let v4_total = (v4_header.len() + v4_tbl.len() + v4_pay.len()) as u32;
        v4_header[0..4].copy_from_slice(&v4_total.to_be_bytes());
        let mut v4_bytes = v4_header;
        v4_bytes.extend_from_slice(&v4_tbl);
        v4_bytes.extend_from_slice(&v4_pay);

        let parsed_v4 = parse_icc_profile(&v4_bytes).expect("parse v4 mluc + para3 .icc");
        assert_eq!(parsed_v4.description.as_deref(), Some("Fuji GFX v4"));
        let v4_lut = parsed_v4.bake_profile_lut();
        let srgb_mid = v4_lut.sample_trilinear([0.5, 0.5, 0.5]);
        // sRGB TRC maps 0.5 -> ~0.2140 linear
        assert!(
            (srgb_mid[0] - 0.214).abs() < 5e-3 && (srgb_mid[0] - srgb_mid[2]).abs() < 1e-3,
            "v4 para3 sRGB TRC expected ~0.214, got {srgb_mid:?}"
        );

        // 5. ICC mft1 (8-bit 3D CLUT) parses and bakes cleanly.
        let grid = 5usize;
        let mut mft1 = b"mft1\0\0\0\0".to_vec();
        mft1.extend_from_slice(&[3, 3, grid as u8, 0]);
        for v in [1.0_f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
            mft1.extend_from_slice(&((v * 65536.0).round() as i32).to_be_bytes());
        }
        for _ in 0..3 {
            for i in 0..=255u8 {
                mft1.push(i);
            }
        }
        let srgb_d50 = [
            0.436_075_f32,
            0.385_065,
            0.143_080,
            0.222_505,
            0.716_879,
            0.060_617,
            0.013_932,
            0.097_105,
            0.714_173,
        ];
        for r in 0..grid {
            let fr = r as f32 / (grid - 1) as f32;
            for g in 0..grid {
                let fg = g as f32 / (grid - 1) as f32;
                for b in 0..grid {
                    let fb = b as f32 / (grid - 1) as f32;
                    let x = srgb_d50[0] * fr + srgb_d50[1] * fg + srgb_d50[2] * fb;
                    let y = srgb_d50[3] * fr + srgb_d50[4] * fg + srgb_d50[5] * fb;
                    let z = srgb_d50[6] * fr + srgb_d50[7] * fg + srgb_d50[8] * fb;
                    for c in [x, y, z] {
                        mft1.push((c * 128.0).round().clamp(0.0, 255.0) as u8);
                    }
                }
            }
        }
        for _ in 0..3 {
            for i in 0..=255u8 {
                mft1.push(i);
            }
        }
        let mut m1_hdr = vec![0u8; 128];
        m1_hdr[12..16].copy_from_slice(b"scnr");
        m1_hdr[16..20].copy_from_slice(b"RGB ");
        m1_hdr[20..24].copy_from_slice(b"XYZ ");
        m1_hdr[36..40].copy_from_slice(b"acsp");
        let mut m1_tbl = Vec::new();
        m1_tbl.extend_from_slice(&1u32.to_be_bytes());
        m1_tbl.extend_from_slice(b"A2B0");
        m1_tbl.extend_from_slice(&(128u32 + 4 + 12).to_be_bytes());
        m1_tbl.extend_from_slice(&(mft1.len() as u32).to_be_bytes());
        let m1_total = (128 + m1_tbl.len() + mft1.len()) as u32;
        m1_hdr[0..4].copy_from_slice(&m1_total.to_be_bytes());
        let mut m1_bytes = m1_hdr;
        m1_bytes.extend_from_slice(&m1_tbl);
        m1_bytes.extend_from_slice(&mft1);

        let parsed_m1 = parse_icc_profile(&m1_bytes).expect("parse mft1 .icc");
        assert!(parsed_m1.a2b0.as_ref().is_some_and(|c| !c.is_16bit));
        let m1_lut = parsed_m1.bake_profile_lut();
        let m1_mid = m1_lut.sample_trilinear([0.5, 0.5, 0.5]);
        assert!(
            (m1_mid[0] - 0.5).abs() < 0.02
                && (m1_mid[1] - 0.5).abs() < 0.02
                && (m1_mid[2] - 0.5).abs() < 0.02,
            "mft1 identity expected ~0.5, got {m1_mid:?}"
        );
    }

    #[test]
    fn user_profile_store_security_cache_invalidation_and_camera_matching() {
        let dir = unique_tmp_dir("store");
        let src_dir = unique_tmp_dir("src");

        // Import synthetic .dcp matched to Sony ILCE-7RM5 and synthetic .cube.
        let dcp_src = src_dir.join("sony_vivid.dcp");
        std::fs::write(
            &dcp_src,
            build_synthetic_dcp(false, "SONY ILCE-7RM5", "Sony Vivid"),
        )
        .unwrap();
        let dcp_desc = import_user_profile_in_dir(&dir, &dcp_src, None).expect("import .dcp");
        assert_eq!(dcp_desc.id, "user:sony_vivid");
        assert_eq!(dcp_desc.category, ProfileCategory::User);
        assert_eq!(dcp_desc.camera_match.as_deref(), Some("SONY ILCE-7RM5"));

        // Refuses accidental overwrite by default.
        assert!(import_user_profile_in_dir(&dir, &dcp_src, None).is_err());

        // Import .cube and verify cache invalidation when replaced with a different grade.
        let cube_src = src_dir.join("grade.cube");
        std::fs::write(&cube_src, build_synthetic_cube_17("Grade A", 0.80, 1.20)).unwrap();
        let cube_desc =
            import_user_profile_in_dir(&dir, &cube_src, Some("My Look")).expect("import .cube");
        assert_eq!(cube_desc.id, "user:my_look");
        let lut_a = resolve_profile_lut("user:my_look", Some(&dir)).expect("resolve lut A");
        let px_a = lut_a.sample_trilinear([0.2, 0.2, 0.2]);

        std::fs::write(&cube_src, build_synthetic_cube_17("Grade B", 1.25, 0.78)).unwrap();
        import_user_profile_in_dir_with_overwrite(&dir, &cube_src, Some("My Look"), true)
            .expect("overwrite .cube");
        let lut_b = resolve_profile_lut("user:my_look", Some(&dir)).expect("resolve lut B");
        let px_b = lut_b.sample_trilinear([0.2, 0.2, 0.2]);
        assert!(
            px_a[0] > px_a[2] && px_b[2] > px_b[0],
            "cache must invalidate on replaced profile: A={px_a:?} B={px_b:?}"
        );

        // Camera EXIF auto-matching sorts matched Sony .dcp profile to index 0!
        let all = list_all_camera_profiles(Some("Sony"), Some("ILCE-7RM5"), Some(&dir));
        assert_eq!(all[0].id, "user:sony_vivid");
        assert!(all[0].camera_match.is_some());

        // Delete removes file and invalidates cache.
        assert!(delete_user_profile_in_dir(&dir, "user:my_look").unwrap());
        assert!(resolve_profile_lut("user:my_look", Some(&dir)).is_none());

        // Adversarial security checks:
        // 1. Path traversal slugs rejected.
        for bad_slug in [
            "../../etc/passwd",
            "../evil",
            "a/b",
            "a\\b",
            ".hidden",
            "",
            "a\0b",
        ] {
            assert!(
                sanitize_profile_slug(bad_slug).is_err(),
                "slug '{bad_slug}' must fail"
            );
        }
        // 2. Disallowed extensions rejected.
        let bad_ext = src_dir.join("evil.sh");
        std::fs::write(&bad_ext, b"#!/bin/sh").unwrap();
        assert!(import_user_profile_in_dir(&dir, &bad_ext, None).is_err());
        let raw_ext = src_dir.join("photo.dng");
        std::fs::write(&raw_ext, build_synthetic_dcp(false, "Sony", "Test")).unwrap();
        assert!(import_user_profile_in_dir(&dir, &raw_ext, None).is_err());

        // 3. Symlink source rejected.
        #[cfg(unix)]
        {
            let link = src_dir.join("link.dcp");
            std::os::unix::fs::symlink(&dcp_src, &link).unwrap();
            assert!(import_user_profile_in_dir(&dir, &link, Some("Link Test")).is_err());
        }

        // 4. Adversarial binary/text payloads rejected cleanly without panic.
        assert!(parse_cube_lut(b"LUT_3D_SIZE 999\n").is_err());
        assert!(parse_cube_lut(b"LUT_3D_SIZE 2\nNaN 0.0 0.0\n").is_err());
        assert!(parse_cube_lut(b"LUT_3D_SIZE 2\n0 0 0\n").is_err());
        assert!(parse_dcp_profile(b"II\x00\x00\x08\x00\x00\x00").is_err());
        assert!(parse_dcp_profile(b"II").is_err());
        // Oversized HueSatMapDims [500, 500, 500] in synthetic DCP header.
        let mut bad_dcp = b"II\x52\x43\x08\x00\x00\x00\x01\x00".to_vec();
        bad_dcp.extend_from_slice(&50937u16.to_le_bytes());
        bad_dcp.extend_from_slice(&4u16.to_le_bytes());
        bad_dcp.extend_from_slice(&3u32.to_le_bytes());
        bad_dcp.extend_from_slice(&26u32.to_le_bytes());
        bad_dcp.extend_from_slice(&0u32.to_le_bytes());
        for d in [500u32, 500, 500] {
            bad_dcp.extend_from_slice(&d.to_le_bytes());
        }
        assert!(parse_dcp_profile(&bad_dcp).is_err());

        // Invalid ICC magic and truncated ICC.
        assert!(parse_icc_profile(&[0u8; 140]).is_err());
        assert!(parse_icc_profile(b"short").is_err());

        // 5. Duplicate recognized .dcp tag rejected (DoS amplification guard).
        let mut dup_dcp = b"II\x52\x43\x08\x00\x00\x00\x02\x00".to_vec();
        for _ in 0..2 {
            dup_dcp.extend_from_slice(&50778u16.to_le_bytes()); // CalibrationIlluminant1
            dup_dcp.extend_from_slice(&3u16.to_le_bytes()); // SHORT
            dup_dcp.extend_from_slice(&1u32.to_le_bytes());
            dup_dcp.extend_from_slice(&21u16.to_le_bytes());
            dup_dcp.extend_from_slice(&0u16.to_le_bytes());
        }
        dup_dcp.extend_from_slice(&0u32.to_le_bytes());
        assert!(
            parse_dcp_profile(&dup_dcp).is_err(),
            "duplicate .dcp IFD tags must be rejected"
        );

        // 6. Duplicate recognized .icc tag (`A2B0` twice) rejected (DoS amplification guard).
        let mut dup_icc_hdr = vec![0u8; 128];
        dup_icc_hdr[12..16].copy_from_slice(b"scnr");
        dup_icc_hdr[16..20].copy_from_slice(b"RGB ");
        dup_icc_hdr[20..24].copy_from_slice(b"XYZ ");
        dup_icc_hdr[36..40].copy_from_slice(b"acsp");
        let dummy_xyz = b"XYZ \0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";
        let mut dup_icc_tbl = 2u32.to_be_bytes().to_vec();
        let data_off = (128 + 4 + 24) as u32;
        for _ in 0..2 {
            dup_icc_tbl.extend_from_slice(b"rXYZ");
            dup_icc_tbl.extend_from_slice(&data_off.to_be_bytes());
            dup_icc_tbl.extend_from_slice(&(dummy_xyz.len() as u32).to_be_bytes());
        }
        let dup_total = (128 + dup_icc_tbl.len() + dummy_xyz.len()) as u32;
        dup_icc_hdr[0..4].copy_from_slice(&dup_total.to_be_bytes());
        let mut dup_icc = dup_icc_hdr;
        dup_icc.extend_from_slice(&dup_icc_tbl);
        dup_icc.extend_from_slice(dummy_xyz);
        assert!(
            parse_icc_profile(&dup_icc).is_err(),
            "duplicate .icc tag signatures must be rejected"
        );

        // 7. Identity user profile caches `None` cleanly (and overwriting non-identity -> identity evicts old LUT).
        let mut id_cube = String::from("TITLE \"Identity 2\"\nLUT_3D_SIZE 2\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    id_cube.push_str(&format!("{r}.0 {g}.0 {b}.0\n"));
                }
            }
        }
        let id_src = src_dir.join("id_test.cube");
        std::fs::write(&id_src, id_cube.as_bytes()).unwrap();
        let id_desc = import_user_profile_in_dir(&dir, &id_src, Some("Id Cache")).unwrap();
        assert_eq!(id_desc.id, "user:id_cache");
        assert!(resolve_profile_lut("user:id_cache", Some(&dir)).is_none());
        // Verify entry was inserted into cache as `None` (no repeated disk rebake).
        let cached_path = dir.join("id_cache.cube");
        {
            let guard = user_lut_cache().lock().unwrap();
            let entry = guard
                .get(&cached_path)
                .expect("identity user profile must be cached as None");
            assert!(entry.lut.is_none());
        }

        // 8. Destination symlink guard on import (both overwrite=false and overwrite=true).
        #[cfg(unix)]
        {
            let dest_link = dir.join("symlink_dest.cube");
            std::os::unix::fs::symlink(&id_src, &dest_link).unwrap();
            assert!(
                import_user_profile_in_dir(&dir, &id_src, Some("symlink_dest")).is_err(),
                "import must reject existing symlink at dest when overwrite=false"
            );
            assert!(
                import_user_profile_in_dir_with_overwrite(
                    &dir,
                    &id_src,
                    Some("symlink_dest"),
                    true
                )
                .is_err(),
                "import must reject existing symlink at dest even when overwrite=true"
            );
        }

        // 9. Whole-token camera EXIF auto-matching rejects false-positive substring collisions.
        assert!(camera_profile_matches_exif(
            Some("Sony ILCE-7RM5 Neutral"),
            Some("Sony"),
            Some("ILCE-7RM5")
        ));
        assert!(camera_profile_matches_exif(
            Some("EOS R5"),
            Some("Canon"),
            Some("Canon EOS R5")
        ));
        for (cand, make, model) in [
            ("Canon EOS R5", "Canon", "Canon EOS R50"),
            ("SONY ILCE-7R", "Sony", "ILCE-7RM5"),
            ("Fujifilm X-T3", "FUJIFILM", "X-T30"),
            ("Pro", "Apple", "iPhone 16 Pro"),
        ] {
            assert!(
                !camera_profile_matches_exif(Some(cand), Some(make), Some(model)),
                "false-positive EXIF match: cand='{cand}' should NOT match ({make}, {model})"
            );
        }

        // 10. High-point DCP ProfileToneCurve (4096 pts) bakes fast in O(1) per texel (no DoS).
        let mut pts_4096 = Vec::with_capacity(4096);
        for i in 0..4096 {
            let x = i as f32 / 4095.0;
            pts_4096.push([x, x.powf(0.95)]);
        }
        let big_dcp = DcpProfile {
            tone_curve: Some(pts_4096),
            ..DcpProfile::default()
        };
        let t0 = std::time::Instant::now();
        let big_lut = big_dcp.bake_profile_lut();
        assert!(
            t0.elapsed().as_millis() < 250,
            "4096-pt DCP bake took too long: {:?}",
            t0.elapsed()
        );
        assert!(!big_lut.is_identity());

        // 11. ICC mft2 LabD50 legacy v2 16-bit encoding preserves exact neutral white & mid-gray!
        let build_const_lab_mft2 = |l_u16: u16, a_u16: u16, b_u16: u16| -> Vec<u8> {
            let mut mft = b"mft2\0\0\0\0".to_vec();
            mft.extend_from_slice(&[3, 3, 2, 0]);
            for v in [1.0_f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
                mft.extend_from_slice(&((v * 65536.0).round() as i32).to_be_bytes());
            }
            mft.extend_from_slice(&2u16.to_be_bytes());
            mft.extend_from_slice(&2u16.to_be_bytes());
            for _ in 0..3 {
                mft.extend_from_slice(&0u16.to_be_bytes());
                mft.extend_from_slice(&65535u16.to_be_bytes());
            }
            for _ in 0..8 {
                mft.extend_from_slice(&l_u16.to_be_bytes());
                mft.extend_from_slice(&a_u16.to_be_bytes());
                mft.extend_from_slice(&b_u16.to_be_bytes());
            }
            for _ in 0..3 {
                mft.extend_from_slice(&0u16.to_be_bytes());
                mft.extend_from_slice(&65535u16.to_be_bytes());
            }
            let mut hdr = vec![0u8; 128];
            hdr[12..16].copy_from_slice(b"scnr");
            hdr[16..20].copy_from_slice(b"RGB ");
            hdr[20..24].copy_from_slice(b"Lab ");
            hdr[36..40].copy_from_slice(b"acsp");
            let mut tbl = 1u32.to_be_bytes().to_vec();
            tbl.extend_from_slice(b"A2B0");
            tbl.extend_from_slice(&(128u32 + 4 + 12).to_be_bytes());
            tbl.extend_from_slice(&(mft.len() as u32).to_be_bytes());
            let total = (128 + tbl.len() + mft.len()) as u32;
            hdr[0..4].copy_from_slice(&total.to_be_bytes());
            hdr.extend_from_slice(&tbl);
            hdr.extend_from_slice(&mft);
            hdr
        };

        // White (0xFF00, 0x8000, 0x8000) -> L*=100, a*=0, b*=0 -> linear sRGB [1.0, 1.0, 1.0]
        let white_icc = build_const_lab_mft2(0xFF00, 0x8000, 0x8000);
        let white_lut = parse_icc_profile(&white_icc).unwrap().bake_profile_lut();
        let white = white_lut.sample_trilinear([0.5, 0.5, 0.5]);
        let white_spread =
            white[0].max(white[1]).max(white[2]) - white[0].min(white[1]).min(white[2]);
        assert!(
            white_spread < 1e-4 && (white[1] - 1.0).abs() < 1e-3,
            "mft2 LabD50 white (0xFF00,0x8000,0x8000) drifted: {white:?} spread={white_spread}"
        );

        // Mid-gray (0x7F80, 0x8000, 0x8000) -> L*=50, a*=0, b*=0 -> linear sRGB Y = ((50+16)/116)^3 ≈ 0.1841865
        let gray_icc = build_const_lab_mft2(0x7F80, 0x8000, 0x8000);
        let gray_lut = parse_icc_profile(&gray_icc).unwrap().bake_profile_lut();
        let mid_gray = gray_lut.sample_trilinear([0.5, 0.5, 0.5]);
        let mid_spread = mid_gray[0].max(mid_gray[1]).max(mid_gray[2])
            - mid_gray[0].min(mid_gray[1]).min(mid_gray[2]);
        assert!(
            mid_spread < 1e-4 && (mid_gray[1] - 0.184_186_5).abs() < 1e-3,
            "mft2 LabD50 mid-gray (0x7F80,0x8000,0x8000) drifted: {mid_gray:?} spread={mid_spread}"
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&src_dir);
    }
}
