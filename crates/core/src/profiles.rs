//! Camera profiles, Bradford dual-illuminant matrix calibration, and unified
//! `33×33×33` 3D LUT color engine shared 1:1 between GPU (`shaders/develop.wgsl`)
//! and CPU (`CompiledRecipe` / `crates/decode/src/export.rs`).
//!
//! All bundled profiles are original MIT-licensed analytical color science baked
//! cleanly into `33³` perceptual-shaper `ProfileLut3D` tables in `<1ms`.

use std::sync::{Arc, OnceLock};

use half::f16;
use serde::{Deserialize, Serialize};

/// Edge dimension of the unified GPU/CPU camera profile 3D LUT (`33×33×33` texels).
pub const PROFILE_LUT_SIZE: usize = 33;

/// Default camera profile identifier (`"auto"` — neutral identity on raster/demo images).
pub const DEFAULT_CAMERA_PROFILE: &str = "auto";

/// Default camera profile blend strength (`1.0`).
pub const DEFAULT_PROFILE_STRENGTH: f32 = 1.0;

/// Upper linear domain covered directly by the log2 shaper (`+6 EV` above diffuse white).
pub const LUT_SHAPER_MAX_LINEAR: f32 = 64.0;

/// Smooth linear toe knee for the log2 LUT shaper (`0.005` linear).
pub const LUT_SHAPER_KNEE: f32 = 0.005;

/// Exact normalization constant `log2(1.0 + 64.0 / 0.005)` so `lut_shaper_forward(64.0) == 1.0`.
pub const LUT_SHAPER_LOG_DENOM: f32 = 13.643_969;

/// Standard CIE Illuminant A correlated color temperature (Kelvin).
pub const ILLUMINANT_STD_A_KELVIN: f32 = 2856.0;

/// Standard CIE Illuminant D65 correlated color temperature (Kelvin).
pub const ILLUMINANT_D65_KELVIN: f32 = 6504.0;

/// Default camera profile helper for `serde(default = ...)`.
pub fn default_camera_profile() -> String {
    DEFAULT_CAMERA_PROFILE.to_owned()
}

/// Default camera profile strength helper for `serde(default = ...)`.
pub const fn default_profile_strength() -> f32 {
    DEFAULT_PROFILE_STRENGTH
}

/// Organizational category for camera calibration and film simulation profiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileCategory {
    Calibration,
    FilmSimulation,
    User,
}

impl ProfileCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Calibration => "calibration",
            Self::FilmSimulation => "film_simulation",
            Self::User => "user",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Calibration => "Camera Calibration",
            Self::FilmSimulation => "Film Simulation",
            Self::User => "User Profiles",
        }
    }
}

/// Camera RAW dual-illuminant calibration metadata extracted from DNG/RAW headers.
///
/// Matrices are stored in row-major `3×3` order matching DNG `ColorMatrix1`/`ColorMatrix2`
/// (`CIE XYZ -> Camera RGB`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CameraCalibration {
    #[serde(default)]
    pub make: String,
    #[serde(default)]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix_d65: Option<[f32; 9]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix_std_a: Option<[f32; 9]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_shot_wb: Option<[f32; 3]>,
}

/// Interpolates a camera's dual-illuminant calibration matrix (`matrix_std_a` at `2856K`
/// and `matrix_d65` at `6504K`) linearly in inverse-mired space (`1e6 / K`).
///
/// Falls back cleanly to `matrix_d65` when `matrix_std_a` is absent (or `matrix_std_a`
/// when `matrix_d65` is absent), and returns `None` if neither matrix is available or valid.
pub fn interpolate_camera_matrix(cal: &CameraCalibration, kelvin: u32) -> Option<[f32; 9]> {
    let d65 = cal.matrix_d65.filter(is_valid_matrix3);
    let std_a = cal.matrix_std_a.filter(is_valid_matrix3);

    match (std_a, d65) {
        (Some(ma), Some(md)) => {
            let k = (kelvin.clamp(1_500, 50_000)) as f64;
            let mired_a = 1_000_000.0 / (ILLUMINANT_STD_A_KELVIN as f64);
            let mired_d65 = 1_000_000.0 / (ILLUMINANT_D65_KELVIN as f64);
            let mired_k = 1_000_000.0 / k;
            let t = ((mired_a - mired_k) / (mired_a - mired_d65)).clamp(0.0, 1.0) as f32;
            let inv_t = 1.0 - t;
            let mut out = [0.0_f32; 9];
            for i in 0..9 {
                out[i] = ma[i] * inv_t + md[i] * t;
            }
            Some(out)
        }
        (None, Some(md)) => Some(md),
        (Some(ma), None) => Some(ma),
        (None, None) => None,
    }
}

/// Computes the CIE XYZ whitepoint (normalized to `Y = 1.0`) for a correlated color
/// temperature `kelvin` using Kang et al. (2002) Planckian locus / CIE D-series approximation.
pub fn kelvin_to_xyz_whitepoint(kelvin: u32) -> [f32; 3] {
    let t = (kelvin.clamp(1_667, 25_000)) as f64;
    let t2 = t * t;
    let t3 = t2 * t;

    let x = if t <= 4_000.0 {
        -0.2661239e9 / t3 - 0.2343589e6 / t2 + 0.8776956e3 / t + 0.179910
    } else {
        -3.0258469e9 / t3 + 2.1070379e6 / t2 + 0.2226347e3 / t + 0.240390
    };
    let x2 = x * x;
    let x3 = x2 * x;
    let y = if t <= 2_222.0 {
        -1.1063814 * x3 - 1.34811020 * x2 + 2.18555832 * x - 0.20219683
    } else if t <= 4_000.0 {
        -0.9549476 * x3 - 1.37418593 * x2 + 2.09137015 * x - 0.16748867
    } else {
        3.0817580 * x3 - 5.87338670 * x2 + 3.75112997 * x - 0.37001483
    };

    let y = y.max(1e-6);
    [(x / y) as f32, 1.0, ((1.0 - x - y) / y) as f32]
}

/// Computes the `3×3` Bradford chromatic adaptation matrix transforming CIE XYZ tristimulus
/// values from `src_white_xyz` to `dst_white_xyz` (row-major).
pub fn bradford_adaptation_matrix(src_white_xyz: [f32; 3], dst_white_xyz: [f32; 3]) -> [f32; 9] {
    const M_BRADFORD: [f32; 9] = [
        0.8951, 0.2664, -0.1614, //
        -0.7502, 1.7135, 0.0367, //
        0.0389, -0.0685, 1.0296,
    ];
    const M_BRADFORD_INV: [f32; 9] = [
        0.9869929, -0.1470543, 0.1599627, //
        0.4323053, 0.5183603, 0.0492912, //
        -0.0085287, 0.0400428, 0.9684867,
    ];

    let src_cone = mul_mat3_vec3(&M_BRADFORD, src_white_xyz);
    let dst_cone = mul_mat3_vec3(&M_BRADFORD, dst_white_xyz);
    let scale = [
        if src_cone[0].abs() > 1e-6 {
            dst_cone[0] / src_cone[0]
        } else {
            1.0
        },
        if src_cone[1].abs() > 1e-6 {
            dst_cone[1] / src_cone[1]
        } else {
            1.0
        },
        if src_cone[2].abs() > 1e-6 {
            dst_cone[2] / src_cone[2]
        } else {
            1.0
        },
    ];

    let scaled = [
        M_BRADFORD[0] * scale[0],
        M_BRADFORD[1] * scale[0],
        M_BRADFORD[2] * scale[0],
        M_BRADFORD[3] * scale[1],
        M_BRADFORD[4] * scale[1],
        M_BRADFORD[5] * scale[1],
        M_BRADFORD[6] * scale[2],
        M_BRADFORD[7] * scale[2],
        M_BRADFORD[8] * scale[2],
    ];
    mul_mat3(&M_BRADFORD_INV, &scaled)
}

/// Computes a Bradford-adapted `Camera RGB (WB-normalized) -> Linear sRGB (D65)` `3×3` matrix
/// for `cal` at color temperature `kelvin` following DNG Spec §6:
/// builds `sRGB -> XYZ_D65 -> XYZ_D50 (Bradford) -> Camera`, normalizes each row to sum `1.0`
/// (canceling per-channel camera WB scaling), and inverts cleanly.
pub fn camera_to_linear_srgb_matrix(cal: &CameraCalibration, kelvin: u32) -> Option<[f32; 9]> {
    // CIE D50 PCS whitepoint (DNG ColorMatrix reference) and CIE D65 (sRGB working space).
    const D50_XYZ: [f32; 3] = [0.96422, 1.00000, 0.82521];
    const D65_XYZ: [f32; 3] = [0.95047, 1.00000, 1.08883];
    // IEC 61966-2-1 Linear sRGB (D65) -> CIE XYZ (D65).
    const SRGB_TO_XYZ_D65: [f32; 9] = [
        0.412_456_4,
        0.357_576_1,
        0.180_437_5, //
        0.212_672_9,
        0.715_152_2,
        0.072_175, //
        0.019_333_9,
        0.119_192,
        0.950_304_1,
    ];

    let xyz_to_cam = interpolate_camera_matrix(cal, kelvin)?;
    let d65_to_d50 = bradford_adaptation_matrix(D65_XYZ, D50_XYZ);
    let srgb_to_xyz_d50 = mul_mat3(&d65_to_d50, &SRGB_TO_XYZ_D65);
    let mut srgb_to_cam = mul_mat3(&xyz_to_cam, &srgb_to_xyz_d50);

    // Row-normalize `srgb_to_cam` so D65 white `[1, 1, 1]` maps to WB-normalized camera white `[1, 1, 1]`.
    for row in 0..3 {
        let base = row * 3;
        let sum = srgb_to_cam[base] + srgb_to_cam[base + 1] + srgb_to_cam[base + 2];
        if !sum.is_finite() || sum.abs() <= 1e-6 {
            return None;
        }
        srgb_to_cam[base] /= sum;
        srgb_to_cam[base + 1] /= sum;
        srgb_to_cam[base + 2] /= sum;
    }
    invert_mat3(&srgb_to_cam)
}

#[inline]
fn det_mat3(m: &[f32; 9]) -> f32 {
    let c00 = m[4] * m[8] - m[5] * m[7];
    let c01 = m[5] * m[6] - m[3] * m[8];
    let c02 = m[3] * m[7] - m[4] * m[6];
    m[0] * c00 + m[1] * c01 + m[2] * c02
}

#[inline]
fn is_valid_matrix3(m: &[f32; 9]) -> bool {
    if !m.iter().all(|v| v.is_finite()) {
        return false;
    }
    let det = det_mat3(m);
    det.is_finite() && det.abs() > 1e-9
}

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

fn invert_mat3(m: &[f32; 9]) -> Option<[f32; 9]> {
    let c00 = m[4] * m[8] - m[5] * m[7];
    let c01 = m[5] * m[6] - m[3] * m[8];
    let c02 = m[3] * m[7] - m[4] * m[6];
    let det = m[0] * c00 + m[1] * c01 + m[2] * c02;
    if !det.is_finite() || det.abs() <= 1e-9 {
        return None;
    }
    let inv_det = 1.0 / det;
    let inv = [
        c00 * inv_det,
        (m[2] * m[7] - m[1] * m[8]) * inv_det,
        (m[1] * m[5] - m[2] * m[4]) * inv_det,
        c01 * inv_det,
        (m[0] * m[8] - m[2] * m[6]) * inv_det,
        (m[2] * m[3] - m[0] * m[5]) * inv_det,
        c02 * inv_det,
        (m[1] * m[6] - m[0] * m[7]) * inv_det,
        (m[0] * m[4] - m[1] * m[3]) * inv_det,
    ];
    is_valid_matrix3(&inv).then_some(inv)
}

/// Forward perceptual log2 shaper mapping scene-referred linear light `[0.0, 64.0]`
/// smoothly into `[0.0, 1.0]` with uniform stop spacing (~2.35 cells/stop across SDR & `+6 EV` HDR).
///
/// Matches `lut_shaper_forward` in `shaders/develop.wgsl` bit-for-bit.
#[inline]
pub fn lut_shaper_forward(x: f32) -> f32 {
    let v = if x.is_finite() { x.max(0.0) } else { 0.0 };
    ((1.0 + v / LUT_SHAPER_KNEE).log2() / LUT_SHAPER_LOG_DENOM).clamp(0.0, 1.0)
}

/// Inverse perceptual log2 shaper mapping `[0.0, 1.0]` back to scene-referred linear
/// light `[0.0, 64.0]`.
///
/// Matches `lut_shaper_inverse` in `shaders/develop.wgsl` bit-for-bit.
#[inline]
pub fn lut_shaper_inverse(s: f32) -> f32 {
    let sc = if s.is_finite() {
        s.clamp(0.0, 1.0)
    } else {
        0.0
    };
    (LUT_SHAPER_KNEE * (2.0_f32.powf(sc * LUT_SHAPER_LOG_DENOM) - 1.0)).max(0.0)
}

/// 3D color lookup table stored in perceptual shaper domain (`[0.0, 1.0]^3`) for exact
/// trilinear interpolation parity between CPU (`sample_trilinear`) and GPU `Rgba16Float`
/// `texture_3d<f32>` (`textureSampleLevel`).
///
/// Data layout is R-fastest (`x = r`, `y = g`, `z = b`, index `(b * size + g) * size + r`),
/// matching both `wgpu::TextureDimension::D3` and Adobe `.cube` 3D LUT ordering.
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileLut3D {
    pub size: usize,
    pub data: Vec<[f32; 3]>,
}

impl ProfileLut3D {
    /// Constructs a strict identity 3D LUT of edge length `size` (minimum `2`).
    pub fn identity(size: usize) -> Self {
        let size = size.max(2);
        let denom = (size - 1) as f32;
        let mut data = Vec::with_capacity(size * size * size);
        for b in 0..size {
            let sb = b as f32 / denom;
            for g in 0..size {
                let sg = g as f32 / denom;
                for r in 0..size {
                    let sr = r as f32 / denom;
                    data.push([sr, sg, sb]);
                }
            }
        }
        Self { size, data }
    }

    /// Bakes a `size³` 3D LUT from a scene-referred linear RGB transform `f`.
    pub fn from_linear_fn(size: usize, f: impl Fn([f32; 3]) -> [f32; 3]) -> Self {
        let size = size.max(2);
        let denom = (size - 1) as f32;
        let mut data = Vec::with_capacity(size * size * size);
        for b in 0..size {
            let sb = b as f32 / denom;
            let lb = lut_shaper_inverse(sb);
            for g in 0..size {
                let sg = g as f32 / denom;
                let lg = lut_shaper_inverse(sg);
                for r in 0..size {
                    let sr = r as f32 / denom;
                    let lr = lut_shaper_inverse(sr);
                    let mapped = f([lr, lg, lb]);
                    data.push([
                        lut_shaper_forward(mapped[0]),
                        lut_shaper_forward(mapped[1]),
                        lut_shaper_forward(mapped[2]),
                    ]);
                }
            }
        }
        Self { size, data }
    }

    /// Returns `true` when every lattice point matches the identity cube within `1e-5`.
    pub fn is_identity(&self) -> bool {
        if self.size < 2 || self.data.len() != self.size * self.size * self.size {
            return true;
        }
        let denom = (self.size - 1) as f32;
        let mut idx = 0usize;
        for b in 0..self.size {
            let sb = b as f32 / denom;
            for g in 0..self.size {
                let sg = g as f32 / denom;
                for r in 0..self.size {
                    let sr = r as f32 / denom;
                    let p = self.data[idx];
                    if (p[0] - sr).abs() > 1e-5
                        || (p[1] - sg).abs() > 1e-5
                        || (p[2] - sb).abs() > 1e-5
                    {
                        return false;
                    }
                    idx += 1;
                }
            }
        }
        true
    }

    /// Evaluates a scene-referred linear RGB pixel through the 3D LUT using exact
    /// perceptual-shaper trilinear interpolation matching `apply_profile_lut` in `develop.wgsl`.
    #[inline]
    pub fn sample_trilinear(&self, rgb: [f32; 3]) -> [f32; 3] {
        let r_in = if rgb[0].is_finite() {
            rgb[0].max(0.0)
        } else {
            0.0
        };
        let g_in = if rgb[1].is_finite() {
            rgb[1].max(0.0)
        } else {
            0.0
        };
        let b_in = if rgb[2].is_finite() {
            rgb[2].max(0.0)
        } else {
            0.0
        };

        let n = self.size;
        if n < 2 || self.data.len() < n * n * n {
            return [r_in, g_in, b_in];
        }

        let sr = lut_shaper_forward(r_in);
        let sg = lut_shaper_forward(g_in);
        let sb = lut_shaper_forward(b_in);

        let max_i = (n - 1) as f32;
        let fx = sr * max_i;
        let fy = sg * max_i;
        let fz = sb * max_i;

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

        let mut sampled = [0.0_f32; 3];
        for ch in 0..3 {
            let c00 = c000[ch] + (c100[ch] - c000[ch]) * tx;
            let c10 = c010[ch] + (c110[ch] - c010[ch]) * tx;
            let c01 = c001[ch] + (c101[ch] - c001[ch]) * tx;
            let c11 = c011[ch] + (c111[ch] - c011[ch]) * tx;
            let c0 = c00 + (c10 - c00) * ty;
            let c1 = c01 + (c11 - c01) * ty;
            sampled[ch] = c0 + (c1 - c0) * tz;
        }

        let ex_r = (r_in - LUT_SHAPER_MAX_LINEAR).max(0.0);
        let ex_g = (g_in - LUT_SHAPER_MAX_LINEAR).max(0.0);
        let ex_b = (b_in - LUT_SHAPER_MAX_LINEAR).max(0.0);

        [
            lut_shaper_inverse(sampled[0]) + ex_r,
            lut_shaper_inverse(sampled[1]) + ex_g,
            lut_shaper_inverse(sampled[2]) + ex_b,
        ]
    }

    /// Packs this 3D LUT into interleaved `Rgba16Float` (`4 × f16` per texel) ready
    /// for direct upload to `wgpu::TextureDimension::D3`.
    pub fn to_rgba16f(&self) -> Vec<f16> {
        let mut out = Vec::with_capacity(self.data.len() * 4);
        for &[r, g, b] in &self.data {
            let cr = if r.is_finite() {
                r.clamp(0.0, 1.0)
            } else {
                0.0
            };
            let cg = if g.is_finite() {
                g.clamp(0.0, 1.0)
            } else {
                0.0
            };
            let cb = if b.is_finite() {
                b.clamp(0.0, 1.0)
            } else {
                0.0
            };
            out.push(f16::from_f32(cr));
            out.push(f16::from_f32(cg));
            out.push(f16::from_f32(cb));
            out.push(f16::ONE);
        }
        out
    }
}

/// Static specification of a bundled MIT-licensed camera profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltinCameraProfileSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub category: ProfileCategory,
    pub description: &'static str,
    pub camera_match: Option<&'static str>,
}

/// Serializable descriptor exposed across GUI, `rd schema`, and MCP.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraProfileDescriptor {
    pub id: String,
    pub name: String,
    pub category: ProfileCategory,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_match: Option<String>,
}

impl From<&BuiltinCameraProfileSpec> for CameraProfileDescriptor {
    fn from(spec: &BuiltinCameraProfileSpec) -> Self {
        Self {
            id: spec.id.to_owned(),
            name: spec.name.to_owned(),
            category: spec.category,
            description: spec.description.to_owned(),
            camera_match: spec.camera_match.map(str::to_owned),
        }
    }
}

/// All 14 bundled MIT-licensed camera calibration & film simulation profiles.
pub const BUILTIN_CAMERA_PROFILES: &[BuiltinCameraProfileSpec] = &[
    // ── Calibration Bases (5) ────────────────────────────────────────────────
    BuiltinCameraProfileSpec {
        id: "auto",
        name: "Auto (Camera Matched)",
        category: ProfileCategory::Calibration,
        description: "Camera-matched dual-illuminant RAW calibration (strict neutral identity on raster and demo frames).",
        camera_match: Some("*"),
    },
    BuiltinCameraProfileSpec {
        id: "pro_standard",
        name: "Rune ProStandard",
        category: ProfileCategory::Calibration,
        description: "OKLab hue-invariant contrast with skin and foliage gamut protection preventing red-to-orange and blue-to-purple hue drift.",
        camera_match: None,
    },
    BuiltinCameraProfileSpec {
        id: "camera_generic",
        name: "Camera Generic",
        category: ProfileCategory::Calibration,
        description: "Standard neutral camera calibration curve with balanced shadow toe and clean highlight roll-off.",
        camera_match: None,
    },
    BuiltinCameraProfileSpec {
        id: "apple_raw",
        name: "Apple Native RAW",
        category: ProfileCategory::Calibration,
        description: "Balanced Apple RAW photographic tone response with open shadows, crisp midtones, and smooth HDR roll-off.",
        camera_match: Some("Apple"),
    },
    BuiltinCameraProfileSpec {
        id: "linear_scientific",
        name: "Linear Scientific",
        category: ProfileCategory::Calibration,
        description: "Flat colorimetric linear reproduction with zero creative tone or chroma shaping.",
        camera_match: None,
    },
    // ── Curated Film & Camera Simulations (9) ────────────────────────────────
    BuiltinCameraProfileSpec {
        id: "portrait_natural",
        name: "Portrait Natural",
        category: ProfileCategory::FilmSimulation,
        description: "Soft shadow transition, creamy highlight shoulder, and refined warm-peach skin tone continuity.",
        camera_match: None,
    },
    BuiltinCameraProfileSpec {
        id: "landscape_crisp",
        name: "Landscape Crisp",
        category: ProfileCategory::FilmSimulation,
        description: "Crisp micro-contrast with deep azure skies, lush emerald foliage, and clean snow/cloud separation.",
        camera_match: None,
    },
    BuiltinCameraProfileSpec {
        id: "editorial_neutral",
        name: "Editorial Neutral",
        category: ProfileCategory::FilmSimulation,
        description: "Restrained low-contrast editorial base with lifted matte blacks and subtle cool-to-warm tonal balance.",
        camera_match: None,
    },
    BuiltinCameraProfileSpec {
        id: "classic_chrome",
        name: "Classic Chrome",
        category: ProfileCategory::FilmSimulation,
        description: "Documentary slide-inspired look with firm shadows, deep reds, and desaturated slate-cyan blues and greens.",
        camera_match: None,
    },
    BuiltinCameraProfileSpec {
        id: "vivid_velvia",
        name: "Vivid Velvia",
        category: ProfileCategory::FilmSimulation,
        description: "High-impact slide emulsion with deep blacks, bold contrast, and vivid gamut-protected greens, blues, and reds.",
        camera_match: None,
    },
    BuiltinCameraProfileSpec {
        id: "soft_astia",
        name: "Soft Astia",
        category: ProfileCategory::FilmSimulation,
        description: "Delicate fashion/portrait slide response with smooth pastels, faithful skin tones, and luminous skies.",
        camera_match: None,
    },
    BuiltinCameraProfileSpec {
        id: "acros_mono",
        name: "Acros Monochrome",
        category: ProfileCategory::FilmSimulation,
        description: "Orthopanchromatic silver-gelatin monochrome with rich shadow depth, clean skin lift, and dramatic dark skies.",
        camera_match: None,
    },
    BuiltinCameraProfileSpec {
        id: "warm_portra",
        name: "Warm Portra 400",
        category: ProfileCategory::FilmSimulation,
        description: "Warm golden-amber color negative emulsion with gentle shadow lift, soft greens, and wide highlight latitude.",
        camera_match: None,
    },
    BuiltinCameraProfileSpec {
        id: "tungsten_cine",
        name: "Tungsten Cine 800T",
        category: ProfileCategory::FilmSimulation,
        description: "Cinema tungsten 800T balance featuring cool cyan-teal shadows and incandescent amber-gold highlights.",
        camera_match: None,
    },
];

/// Returns descriptors for all 14 bundled MIT-licensed camera profiles.
pub fn builtin_camera_profiles() -> Vec<CameraProfileDescriptor> {
    BUILTIN_CAMERA_PROFILES
        .iter()
        .map(CameraProfileDescriptor::from)
        .collect()
}

/// Looks up a bundled camera profile by canonical ID, slug alias, or display name (case-insensitive).
pub fn find_builtin_profile(id: &str) -> Option<CameraProfileDescriptor> {
    let trimmed = id.trim();
    let slug = trimmed.to_ascii_lowercase().replace(['-', ' '], "_");
    BUILTIN_CAMERA_PROFILES
        .iter()
        .find(|p| p.id == slug || p.name.eq_ignore_ascii_case(trimmed))
        .map(CameraProfileDescriptor::from)
}

/// Normalizes a profile identifier to its canonical builtin slug (or `"user:<slug>"`) when
/// recognized, or returns the trimmed identifier (defaulting empty strings to `"auto"`).
pub fn canonical_profile_id(id: &str) -> String {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return DEFAULT_CAMERA_PROFILE.to_owned();
    }
    if let Some(desc) = find_builtin_profile(trimmed) {
        return desc.id;
    }
    if let Some(rest) = trimmed.strip_prefix(crate::profile_io::USER_PROFILE_PREFIX)
        && let Ok(slug) = crate::profile_io::sanitize_profile_slug(rest)
    {
        return format!("{}{slug}", crate::profile_io::USER_PROFILE_PREFIX);
    }
    trimmed.to_owned()
}

/// Returns `true` if `id` resolves to a valid builtin profile or a valid `"user:<slug>"` ID.
pub fn is_valid_camera_profile_id(id: &str) -> bool {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return false;
    }
    if find_builtin_profile(trimmed).is_some() {
        return true;
    }
    if let Some(rest) = trimmed.strip_prefix(crate::profile_io::USER_PROFILE_PREFIX) {
        return crate::profile_io::sanitize_profile_slug(rest).is_ok();
    }
    false
}

/// Bakes a `33×33×33` [`ProfileLut3D`] for any bundled camera profile ID in `<1ms`.
///
/// Unknown profile IDs fall back cleanly to an identity LUT.
pub fn bake_builtin_profile_lut(id: &str) -> ProfileLut3D {
    let canonical = canonical_profile_id(id);
    match canonical.as_str() {
        "auto" | "linear_scientific" => ProfileLut3D::identity(PROFILE_LUT_SIZE),
        "pro_standard" => ProfileLut3D::from_linear_fn(PROFILE_LUT_SIZE, eval_pro_standard),
        "camera_generic" => ProfileLut3D::from_linear_fn(PROFILE_LUT_SIZE, eval_camera_generic),
        "apple_raw" => ProfileLut3D::from_linear_fn(PROFILE_LUT_SIZE, eval_apple_raw),
        "portrait_natural" => ProfileLut3D::from_linear_fn(PROFILE_LUT_SIZE, eval_portrait_natural),
        "landscape_crisp" => ProfileLut3D::from_linear_fn(PROFILE_LUT_SIZE, eval_landscape_crisp),
        "editorial_neutral" => {
            ProfileLut3D::from_linear_fn(PROFILE_LUT_SIZE, eval_editorial_neutral)
        }
        "classic_chrome" => ProfileLut3D::from_linear_fn(PROFILE_LUT_SIZE, eval_classic_chrome),
        "vivid_velvia" => ProfileLut3D::from_linear_fn(PROFILE_LUT_SIZE, eval_vivid_velvia),
        "soft_astia" => ProfileLut3D::from_linear_fn(PROFILE_LUT_SIZE, eval_soft_astia),
        "acros_mono" => ProfileLut3D::from_linear_fn(PROFILE_LUT_SIZE, eval_acros_mono),
        "warm_portra" => ProfileLut3D::from_linear_fn(PROFILE_LUT_SIZE, eval_warm_portra),
        "tungsten_cine" => ProfileLut3D::from_linear_fn(PROFILE_LUT_SIZE, eval_tungsten_cine),
        _ => ProfileLut3D::identity(PROFILE_LUT_SIZE),
    }
}

/// Returns a lazily cached `Arc<ProfileLut3D>` for any active (non-identity) builtin profile.
///
/// Returns `None` for `"auto"`, `"linear_scientific"`, and unrecognized IDs so CPU/GPU
/// pipelines take their zero-cost fast-path bypass without allocations on 120Hz drags.
pub fn cached_builtin_profile_lut(id: &str) -> Option<Arc<ProfileLut3D>> {
    let canonical = canonical_profile_id(id);
    let slot_idx = match canonical.as_str() {
        "pro_standard" => 0,
        "camera_generic" => 1,
        "apple_raw" => 2,
        "portrait_natural" => 3,
        "landscape_crisp" => 4,
        "editorial_neutral" => 5,
        "classic_chrome" => 6,
        "vivid_velvia" => 7,
        "soft_astia" => 8,
        "acros_mono" => 9,
        "warm_portra" => 10,
        "tungsten_cine" => 11,
        _ => return None,
    };
    static CACHE: [OnceLock<Arc<ProfileLut3D>>; 12] = [const { OnceLock::new() }; 12];
    Some(Arc::clone(CACHE[slot_idx].get_or_init(|| {
        Arc::new(bake_builtin_profile_lut(&canonical))
    })))
}

// ── OKLab / OKLCh Perceptual Color Science Helpers ───────────────────────────

/// Converts linear sRGB (D65) to Björn Ottosson's OKLab `(L, a, b)` (MIT).
#[inline]
pub fn linear_srgb_to_oklab(rgb: [f32; 3]) -> [f32; 3] {
    let r = rgb[0].max(0.0);
    let g = rgb[1].max(0.0);
    let b = rgb[2].max(0.0);

    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).max(0.0);
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).max(0.0);
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).max(0.0);

    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();

    [
        0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_,
        1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_,
        0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_,
    ]
}

#[inline]
fn oklab_to_linear_srgb_unclamped(lab: [f32; 3]) -> [f32; 3] {
    let l_ = lab[0] + 0.396_337_78 * lab[1] + 0.215_803_76 * lab[2];
    let m_ = lab[0] - 0.105_561_346 * lab[1] - 0.063_854_17 * lab[2];
    let s_ = lab[0] - 0.089_484_18 * lab[1] - 1.291_485_5 * lab[2];

    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;

    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

/// Converts OKLab `(L, a, b)` to linear sRGB, projecting any negative-channel excursions
/// back onto the non-negative linear sRGB gamut boundary along constant `(L, hue)` so
/// hue never drifts under contrast or saturation shaping.
#[inline]
pub fn oklab_to_linear_srgb_gamut_safe(lab: [f32; 3]) -> [f32; 3] {
    let l = lab[0].max(0.0);
    if l <= 1e-6 {
        return [0.0, 0.0, 0.0];
    }
    let raw = oklab_to_linear_srgb_unclamped([l, lab[1], lab[2]]);
    if raw[0] >= 0.0 && raw[1] >= 0.0 && raw[2] >= 0.0 {
        return raw;
    }

    // Binary search maximum chroma scale alpha in [0, 1] keeping all channels >= 0 (exact hue invariance).
    let mut lo = 0.0_f32;
    let mut hi = 1.0_f32;
    for _ in 0..12 {
        let mid = 0.5 * (lo + hi);
        let c = oklab_to_linear_srgb_unclamped([l, lab[1] * mid, lab[2] * mid]);
        if c[0] >= 0.0 && c[1] >= 0.0 && c[2] >= 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let safe = oklab_to_linear_srgb_unclamped([l, lab[1] * lo, lab[2] * lo]);
    [safe[0].max(0.0), safe[1].max(0.0), safe[2].max(0.0)]
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn hue_band_weight(hue_deg: f32, center_deg: f32, half_width_deg: f32) -> f32 {
    let d = ((hue_deg - center_deg + 540.0).rem_euclid(360.0) - 180.0).abs();
    let t = (1.0 - d / half_width_deg.max(1.0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn smooth_l_curve(l: f32, contrast: f32, toe_lift: f32, shoulder: f32, pivot: f32) -> f32 {
    let l = l.max(0.0);
    let toe = toe_lift.clamp(0.0, 0.25);
    if l <= 1e-6 {
        return toe;
    }
    let sdr = l.min(1.0);
    let hdr = (l - 1.0).max(0.0);

    // C-infinity finite-slope monotonic S-curve on [0, 1]:
    // q'(x) = -3x^2 + 2(1+p)x - p; with |c| <= 0.45 and p in [0.35, 0.65], shaped'(x) >= 0.35 > 0 everywhere.
    let c = contrast.clamp(-0.45, 0.45);
    let p = pivot.clamp(0.35, 0.65);
    let shaped = (sdr + c * 2.2 * sdr * (1.0 - sdr) * (sdr - p)).clamp(0.0, 1.0);

    // Monotonic shadow floor lift & gentle highlight shoulder compression.
    let lifted = toe + shaped * (1.0 - toe);
    let s = shoulder.clamp(0.0, 0.45);
    let rolled = if s > 1e-5 {
        let knee = 0.72;
        if lifted <= knee {
            lifted
        } else {
            let span = 1.0 - knee;
            let norm = (lifted - knee) / span;
            let comp = norm / (1.0 + s * norm);
            let max_comp = 1.0 / (1.0 + s);
            knee + (comp / max_comp) * span
        }
    } else {
        lifted
    };

    // Match exact C1 derivative at L = 1.0- so SDR->HDR transition has zero slope discontinuity.
    let m1 = (1.0 - toe) * (1.0 - c * 2.2 * (1.0 - p)) / (1.0 + s);
    let hdr_out = (m1 * hdr) / (1.0 + s * 0.5 * hdr);
    (rolled + hdr_out).max(0.0)
}

#[inline]
fn split_tint_lab(l: f32, sh_a: f32, sh_b: f32, hi_a: f32, hi_b: f32) -> (f32, f32) {
    let t = (l / (l + 0.55)).clamp(0.0, 1.0);
    // Keep pure black strictly near neutral so zero stays black and monotonic.
    let black_guard = (l / 0.08).clamp(0.0, 1.0);
    let w_sh = (1.0 - t) * (1.0 - t) * black_guard;
    let w_hi = t * t;
    (sh_a * w_sh + hi_a * w_hi, sh_b * w_sh + hi_b * w_hi)
}

// ── Individual MIT Profile Color Science Evaluators ──────────────────────────

fn eval_pro_standard(rgb: [f32; 3]) -> [f32; 3] {
    let [l, a, b] = linear_srgb_to_oklab(rgb);
    let c = a.hypot(b);
    let h_rad = b.atan2(a);
    let h_deg = h_rad.to_degrees().rem_euclid(360.0);

    // Hue-invariant OKLab lightness S-curve (deep crisp blacks, clean midtones, smooth highlight roll-off).
    let l_out = smooth_l_curve(l, 0.22, 0.0, 0.16, 0.56);

    // Preserve exact OKLab hue (`h_rad` untouched!) while protecting skin & foliage chroma
    // and smoothly rolling off highlight chroma to avoid red->orange / blue->purple clipping skew.
    let mid_w = 4.0 * l.clamp(0.0, 1.0) * (1.0 - l.clamp(0.0, 1.0));
    let hi_desat = 1.0 - 0.18 * ((l - 0.78) / 0.65).clamp(0.0, 1.0);
    let chroma_gate = smoothstep(0.02, 0.09, c);
    let skin_w = hue_band_weight(h_deg, 45.0, 24.0) * chroma_gate;
    let foliage_w = hue_band_weight(h_deg, 138.0, 26.0) * chroma_gate;
    let chroma_scale = (1.0 + 0.06 * mid_w - 0.05 * skin_w + 0.03 * foliage_w) * hi_desat;
    let c_out = (c * chroma_scale).max(0.0);

    oklab_to_linear_srgb_gamut_safe([l_out, c_out * h_rad.cos(), c_out * h_rad.sin()])
}

fn eval_camera_generic(rgb: [f32; 3]) -> [f32; 3] {
    let [l, a, b] = linear_srgb_to_oklab(rgb);
    let l_out = smooth_l_curve(l, 0.10, 0.0, 0.10, 0.55);
    let hi_desat = 1.0 - 0.08 * ((l - 0.85) / 0.70).clamp(0.0, 1.0);
    oklab_to_linear_srgb_gamut_safe([l_out, a * hi_desat, b * hi_desat])
}

fn eval_apple_raw(rgb: [f32; 3]) -> [f32; 3] {
    let [l, a, b] = linear_srgb_to_oklab(rgb);
    // Gentle Apple-like shadow open-up + balanced midtone lift + soft HDR highlight roll-off.
    let shadow_open = l + 0.045 * l.min(1.0) * (1.0 - l.min(1.0)).powi(2);
    let l_out = smooth_l_curve(shadow_open, 0.14, 0.0, 0.24, 0.54);
    let mid_w = 4.0 * l.clamp(0.0, 1.0) * (1.0 - l.clamp(0.0, 1.0));
    let c_scale = 1.0 + 0.08 * mid_w;
    oklab_to_linear_srgb_gamut_safe([l_out, a * c_scale, b * c_scale])
}

fn eval_portrait_natural(rgb: [f32; 3]) -> [f32; 3] {
    let [l, a, b] = linear_srgb_to_oklab(rgb);
    let c = a.hypot(b);
    let h_deg = b.atan2(a).to_degrees().rem_euclid(360.0);

    let skin_w = hue_band_weight(h_deg, 46.0, 28.0);
    let magenta_w = hue_band_weight(h_deg, 340.0, 28.0);
    let green_w = hue_band_weight(h_deg, 142.0, 26.0);

    // Creamy portrait contrast with lifted shadows and softly glowing skin lightness.
    let l_base = smooth_l_curve(l, 0.08, 0.006, 0.26, 0.52);
    let l_out = (l_base + 0.014 * skin_w * (c / 0.12).clamp(0.0, 1.0)).max(0.0);

    // Subtle hue harmony: pull red-leaning skin gently toward warm peach (+2.5°), calm harsh magenta/green.
    let h_new = (h_deg + 2.5 * skin_w).to_radians();
    let c_scale = (1.0 - 0.06 * skin_w - 0.12 * magenta_w - 0.08 * green_w)
        * (1.0 - 0.16 * ((l - 0.80) / 0.60).clamp(0.0, 1.0));
    let c_out = (c * c_scale).max(0.0);

    let (ta, tb) = split_tint_lab(l_out, 0.002, 0.003, 0.003, 0.005);
    oklab_to_linear_srgb_gamut_safe([l_out, c_out * h_new.cos() + ta, c_out * h_new.sin() + tb])
}

fn eval_landscape_crisp(rgb: [f32; 3]) -> [f32; 3] {
    let [l, a, b] = linear_srgb_to_oklab(rgb);
    let c = a.hypot(b);
    let h_deg = b.atan2(a).to_degrees().rem_euclid(360.0);

    let sky_w = hue_band_weight(h_deg, 252.0, 32.0);
    let foliage_w = hue_band_weight(h_deg, 138.0, 30.0);
    let warm_w = hue_band_weight(h_deg, 65.0, 32.0);

    // Crisp micro-contrast with deeper skies and clean bright snow/clouds.
    let l_base = smooth_l_curve(l, 0.28, 0.0, 0.12, 0.56);
    let l_out = (l_base - 0.022 * sky_w * (c / 0.14).clamp(0.0, 1.0)).max(0.0);

    let h_new = (h_deg + 4.0 * foliage_w - 2.0 * sky_w).to_radians();
    let mid_w = 4.0 * l.clamp(0.0, 1.0) * (1.0 - l.clamp(0.0, 1.0));
    let c_scale = 1.0 + 0.12 * mid_w + 0.14 * sky_w + 0.12 * foliage_w + 0.08 * warm_w;
    let c_out = (c * c_scale).max(0.0);

    oklab_to_linear_srgb_gamut_safe([l_out, c_out * h_new.cos(), c_out * h_new.sin()])
}

fn eval_editorial_neutral(rgb: [f32; 3]) -> [f32; 3] {
    let [l, a, b] = linear_srgb_to_oklab(rgb);
    // Low-contrast flat editorial base with subtle lifted matte blacks & restrained chroma.
    let l_out = smooth_l_curve(l, -0.14, 0.014, 0.22, 0.54);
    let c_scale = 0.86;
    let (ta, tb) = split_tint_lab(l_out, -0.002, -0.004, 0.002, 0.004);
    oklab_to_linear_srgb_gamut_safe([l_out, a * c_scale + ta, b * c_scale + tb])
}

fn eval_classic_chrome(rgb: [f32; 3]) -> [f32; 3] {
    let [l, a, b] = linear_srgb_to_oklab(rgb);
    let c = a.hypot(b);
    let h_deg = b.atan2(a).to_degrees().rem_euclid(360.0);

    let red_w = hue_band_weight(h_deg, 26.0, 26.0);
    let green_w = hue_band_weight(h_deg, 140.0, 32.0);
    let cyan_blue_w = hue_band_weight(h_deg, 235.0, 48.0);

    // Documentary contrast: firm shadows, deeper reds & greens, muted cool-cyan blues.
    let l_base = smooth_l_curve(l, 0.30, 0.004, 0.22, 0.56);
    let c_norm = (c / 0.14).clamp(0.0, 1.0);
    let l_out = (l_base - 0.025 * red_w * c_norm - 0.020 * green_w * c_norm).max(0.0);

    let h_new = (h_deg - 10.0 * cyan_blue_w + 5.0 * green_w - 3.0 * red_w).to_radians();
    let c_scale = (0.90 + 0.10 * red_w - 0.24 * green_w - 0.28 * cyan_blue_w).max(0.2);
    let c_out = (c * c_scale).max(0.0);

    let (ta, tb) = split_tint_lab(l_out, -0.003, -0.005, 0.003, 0.007);
    oklab_to_linear_srgb_gamut_safe([l_out, c_out * h_new.cos() + ta, c_out * h_new.sin() + tb])
}

fn eval_vivid_velvia(rgb: [f32; 3]) -> [f32; 3] {
    let [l, a, b] = linear_srgb_to_oklab(rgb);
    let c = a.hypot(b);
    let h_deg = b.atan2(a).to_degrees().rem_euclid(360.0);

    let red_mag_w = hue_band_weight(h_deg, 16.0, 32.0);
    let green_w = hue_band_weight(h_deg, 142.0, 30.0);
    let blue_w = hue_band_weight(h_deg, 260.0, 34.0);

    // Rich slide film contrast with deep velvety shadows and punchy chromaticity.
    let l_base = smooth_l_curve(l, 0.38, 0.0, 0.14, 0.57);
    let c_norm = (c / 0.15).clamp(0.0, 1.0);
    let l_out = (l_base - 0.024 * blue_w * c_norm - 0.015 * red_mag_w * c_norm).max(0.0);

    let h_new = (h_deg - 4.0 * red_mag_w + 5.0 * green_w + 3.0 * blue_w).to_radians();
    let hi_guard = 1.0 - 0.22 * ((l - 0.80) / 0.55).clamp(0.0, 1.0);
    let c_scale = (1.26 + 0.12 * green_w + 0.12 * blue_w + 0.10 * red_mag_w) * hi_guard;
    let c_out = (c * c_scale).max(0.0);

    oklab_to_linear_srgb_gamut_safe([l_out, c_out * h_new.cos(), c_out * h_new.sin()])
}

fn eval_soft_astia(rgb: [f32; 3]) -> [f32; 3] {
    let [l, a, b] = linear_srgb_to_oklab(rgb);
    let c = a.hypot(b);
    let h_deg = b.atan2(a).to_degrees().rem_euclid(360.0);

    let skin_w = hue_band_weight(h_deg, 48.0, 28.0);
    let blue_w = hue_band_weight(h_deg, 250.0, 35.0);

    // Smooth fashion/portrait slide: gentle open shadows, luminous midtones, silky highlight shoulder.
    let l_out = smooth_l_curve(l, 0.12, 0.008, 0.28, 0.50);
    let c_scale =
        (1.08 - 0.08 * skin_w + 0.08 * blue_w) * (1.0 - 0.15 * ((l - 0.82) / 0.60).clamp(0.0, 1.0));
    let c_out = (c * c_scale).max(0.0);
    let h_rad = b.atan2(a);

    oklab_to_linear_srgb_gamut_safe([l_out, c_out * h_rad.cos(), c_out * h_rad.sin()])
}

fn eval_acros_mono(rgb: [f32; 3]) -> [f32; 3] {
    let r = rgb[0].max(0.0);
    let g = rgb[1].max(0.0);
    let b = rgb[2].max(0.0);

    // Orthopanchromatic silver-gelatin spectral sensitivity: warm skin/red-orange lift, deep blue skies.
    let mono_lin = (0.295 * r + 0.665 * g + 0.040 * b).max(0.0);
    let [l, _, _] = linear_srgb_to_oklab([mono_lin, mono_lin, mono_lin]);
    let l_out = smooth_l_curve(l, 0.36, 0.003, 0.18, 0.56);
    let y = l_out * l_out * l_out;
    [y, y, y]
}

fn eval_warm_portra(rgb: [f32; 3]) -> [f32; 3] {
    let [l, a, b] = linear_srgb_to_oklab(rgb);
    let c = a.hypot(b);
    let h_deg = b.atan2(a).to_degrees().rem_euclid(360.0);

    let warm_w = hue_band_weight(h_deg, 55.0, 36.0);
    let green_w = hue_band_weight(h_deg, 138.0, 32.0);
    let blue_w = hue_band_weight(h_deg, 248.0, 38.0);

    // Portra 400 negative latitude: softly lifted shadows, wide creamy highlight roll-off.
    let l_out = smooth_l_curve(l, 0.06, 0.012, 0.30, 0.50);
    let h_new = (h_deg - 4.0 * green_w + 3.0 * warm_w - 6.0 * blue_w).to_radians();
    let c_scale = (0.96 + 0.08 * warm_w - 0.16 * green_w - 0.14 * blue_w)
        * (1.0 - 0.20 * ((l - 0.76) / 0.60).clamp(0.0, 1.0));
    let c_out = (c * c_scale).max(0.0);

    let (ta, tb) = split_tint_lab(l_out, -0.002, 0.006, 0.006, 0.012);
    oklab_to_linear_srgb_gamut_safe([l_out, c_out * h_new.cos() + ta, c_out * h_new.sin() + tb])
}

fn eval_tungsten_cine(rgb: [f32; 3]) -> [f32; 3] {
    let [l, a, b] = linear_srgb_to_oklab(rgb);
    let c = a.hypot(b);
    let h_deg = b.atan2(a).to_degrees().rem_euclid(360.0);

    let amber_w = hue_band_weight(h_deg, 48.0, 34.0);
    let cyan_w = hue_band_weight(h_deg, 225.0, 44.0);

    // Cinema 800T tone curve: gently lifted cinematic blacks, cool teal shadows, glowing tungsten highlights.
    let l_out = smooth_l_curve(l, 0.24, 0.010, 0.24, 0.54);
    let h_new = (h_deg - 6.0 * cyan_w - 4.0 * amber_w).to_radians();
    let c_scale = (0.94 + 0.14 * amber_w + 0.06 * cyan_w)
        * (1.0 - 0.14 * ((l - 0.82) / 0.60).clamp(0.0, 1.0));
    let c_out = (c * c_scale).max(0.0);

    let (ta, tb) = split_tint_lab(l_out, -0.012, -0.016, 0.010, 0.015);
    oklab_to_linear_srgb_gamut_safe([l_out, c_out * h_new.cos() + ta, c_out * h_new.sin() + tb])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bradford_dual_illuminant_interpolation_and_fallback() {
        let m_a = [
            0.85, -0.20, -0.05, //
            -0.40, 1.20, 0.20, //
            -0.08, 0.18, 0.72,
        ];
        let m_d65 = [
            0.65, -0.15, -0.06, //
            -0.46, 1.25, 0.21, //
            -0.09, 0.16, 0.64,
        ];
        let cal = CameraCalibration {
            make: "Sony".to_owned(),
            model: "ILCE-7RM5".to_owned(),
            matrix_d65: Some(m_d65),
            matrix_std_a: Some(m_a),
            as_shot_wb: Some([2.1, 1.0, 1.55]),
        };

        // At 2856K -> exact StdA matrix.
        let at_a = interpolate_camera_matrix(&cal, 2856).expect("2856K matrix");
        for i in 0..9 {
            assert!((at_a[i] - m_a[i]).abs() < 1e-5, "2856K mismatch at {i}");
        }

        // At 6504K -> exact D65 matrix.
        let at_d65 = interpolate_camera_matrix(&cal, 6504).expect("6504K matrix");
        for i in 0..9 {
            assert!((at_d65[i] - m_d65[i]).abs() < 1e-5, "6504K mismatch at {i}");
        }

        // At 4500K -> exact inverse-mired interpolation between StdA and D65.
        let at_4500 = interpolate_camera_matrix(&cal, 4500).expect("4500K matrix");
        let mired_a = 1_000_000.0_f64 / 2856.0;
        let mired_d65 = 1_000_000.0_f64 / 6504.0;
        let mired_4500 = 1_000_000.0_f64 / 4500.0;
        let t = ((mired_a - mired_4500) / (mired_a - mired_d65)) as f32;
        assert!(t > 0.60 && t < 0.70, "expected t ~0.651 at 4500K, got {t}");
        for i in 0..9 {
            let expected = m_a[i] * (1.0 - t) + m_d65[i] * t;
            assert!(
                (at_4500[i] - expected).abs() < 1e-5,
                "4500K mismatch at {i}: got {}, expected {expected}",
                at_4500[i]
            );
        }

        // Single-illuminant fallback when matrix_std_a is absent.
        let d65_only = CameraCalibration {
            matrix_std_a: None,
            ..cal.clone()
        };
        assert_eq!(interpolate_camera_matrix(&d65_only, 3200), Some(m_d65));

        // Clamping below 2000K (1000K) and above 14000K (20000K).
        let at_1000 = interpolate_camera_matrix(&cal, 1000).expect("1000K matrix");
        let at_20000 = interpolate_camera_matrix(&cal, 20_000).expect("20000K matrix");
        for i in 0..9 {
            assert!((at_1000[i] - m_a[i]).abs() < 1e-5);
            assert!((at_20000[i] - m_d65[i]).abs() < 1e-5);
        }

        // Singular matrix guard: degenerate matrix_d65 falls back cleanly to valid matrix_std_a.
        let singular = [1.0, 2.0, 3.0, 2.0, 4.0, 6.0, 3.0, 6.0, 9.0];
        let singular_d65 = CameraCalibration {
            matrix_d65: Some(singular),
            ..cal.clone()
        };
        assert_eq!(interpolate_camera_matrix(&singular_d65, 6504), Some(m_a));
        let both_singular = CameraCalibration {
            matrix_d65: Some(singular),
            matrix_std_a: Some(singular),
            ..cal.clone()
        };
        assert_eq!(interpolate_camera_matrix(&both_singular, 5500), None);

        // Empty calibration returns None cleanly.
        assert_eq!(
            interpolate_camera_matrix(&CameraCalibration::default(), 5500),
            None
        );

        // Bradford adaptation of identical whitepoints is exact identity.
        let d65_wp = [0.95047, 1.0, 1.08883];
        let id_cat = bradford_adaptation_matrix(d65_wp, d65_wp);
        for r in 0..3 {
            for c in 0..3 {
                let expected = if r == c { 1.0 } else { 0.0 };
                assert!((id_cat[r * 3 + c] - expected).abs() < 1e-4);
            }
        }

        // Exact ColorChecker roundtrip on a Sony A7R IV/V-class XYZ(D50)->Cam matrix:
        // WB-normalized camera RGB mapped through camera_to_linear_srgb_matrix must recover sRGB < 1e-4.
        let sony_d65 = [
            0.5271, -0.0712, -0.0347, //
            -0.6153, 1.3653, 0.2763, //
            -0.1601, 0.2366, 0.7242,
        ];
        let sony_cal = CameraCalibration {
            make: "Sony".to_owned(),
            model: "ILCE-7RM4".to_owned(),
            matrix_d65: Some(sony_d65),
            matrix_std_a: None,
            as_shot_wb: Some([2.35, 1.0, 1.62]),
        };
        let cam_to_srgb = camera_to_linear_srgb_matrix(&sony_cal, 6504).expect("sony cam_to_srgb");
        const D50_XYZ: [f32; 3] = [0.96422, 1.00000, 0.82521];
        const D65_XYZ: [f32; 3] = [0.95047, 1.00000, 1.08883];
        const SRGB_TO_XYZ_D65: [f32; 9] = [
            0.412_456_4,
            0.357_576_1,
            0.180_437_5,
            0.212_672_9,
            0.715_152_2,
            0.072_175,
            0.019_333_9,
            0.119_192,
            0.950_304_1,
        ];
        let srgb_to_xyz_d50 = mul_mat3(
            &bradford_adaptation_matrix(D65_XYZ, D50_XYZ),
            &SRGB_TO_XYZ_D65,
        );
        let forward_raw = mul_mat3(&sony_d65, &srgb_to_xyz_d50);
        let cam_white = mul_mat3_vec3(&forward_raw, [1.0, 1.0, 1.0]);
        let patches = [
            [0.45, 0.06, 0.05], // ColorChecker Red
            [0.08, 0.34, 0.09], // Foliage Green
            [0.05, 0.12, 0.42], // Blue
            [0.52, 0.31, 0.21], // Light Skin
            [0.18, 0.18, 0.18], // 18% Gray
            [1.00, 1.00, 1.00], // White
        ];
        for patch in patches {
            let cam_raw = mul_mat3_vec3(&forward_raw, patch);
            let cam_wb = [
                cam_raw[0] / cam_white[0],
                cam_raw[1] / cam_white[1],
                cam_raw[2] / cam_white[2],
            ];
            let recovered = mul_mat3_vec3(&cam_to_srgb, cam_wb);
            for ch in 0..3 {
                assert!(
                    (recovered[ch] - patch[ch]).abs() < 1e-4,
                    "ColorChecker roundtrip error on {patch:?} ch {ch}: got {}, expected {}",
                    recovered[ch],
                    patch[ch]
                );
            }
        }
    }

    #[test]
    fn profile_lut_trilinear_identity_hdr_and_rgba16f_packing() {
        let id = ProfileLut3D::identity(PROFILE_LUT_SIZE);
        assert!(id.is_identity());
        assert_eq!(id.data.len(), PROFILE_LUT_SIZE.pow(3));

        let packed = id.to_rgba16f();
        assert_eq!(packed.len(), PROFILE_LUT_SIZE.pow(3) * 4);
        assert_eq!(packed[3], f16::ONE);

        // Identity trilinear lookup preserves shadows, midtones, and HDR highlights > 1.0 accurately.
        let samples = [
            [0.0, 0.0, 0.0],
            [0.005, 0.012, 0.024],
            [0.18, 0.18, 0.18],
            [0.62, 0.24, 0.08],
            [1.0, 0.5, 0.25],
            [2.4, 1.8, 0.9],
            [12.0, 4.5, 1.2],
            [64.0, 32.0, 16.0],
            [80.0, 10.0, 0.5],
        ];
        for px in samples {
            let out = id.sample_trilinear(px);
            for ch in 0..3 {
                let tol = (px[ch] * 1e-4).max(1e-5);
                assert!(
                    (out[ch] - px[ch]).abs() <= tol,
                    "identity LUT drift on {px:?} ch {ch}: got {}, expected {}",
                    out[ch],
                    px[ch]
                );
            }
        }
    }

    #[test]
    fn all_14_builtin_profiles_finite_monotonic_distinct_and_prostandard_hue_invariant() {
        let all = builtin_camera_profiles();
        assert_eq!(all.len(), 14, "expected 14 bundled MIT camera profiles");

        // Verify unique IDs and lookup roundtrip.
        let mut ids = std::collections::HashSet::new();
        for desc in &all {
            assert!(
                ids.insert(desc.id.clone()),
                "duplicate profile id {}",
                desc.id
            );
            assert_eq!(find_builtin_profile(&desc.id).as_ref(), Some(desc));
            assert_eq!(find_builtin_profile(&desc.name).as_ref(), Some(desc));
        }

        // "auto" and "linear_scientific" are strict identity LUTs.
        assert!(bake_builtin_profile_lut("auto").is_identity());
        assert!(bake_builtin_profile_lut("linear_scientific").is_identity());
        assert!(cached_builtin_profile_lut("auto").is_none());
        assert!(cached_builtin_profile_lut("linear_scientific").is_none());

        let gray_ramp = [0.0, 0.01, 0.04, 0.10, 0.18, 0.35, 0.65, 1.0, 2.5, 8.0];
        let probe_colors = [
            [0.45, 0.08, 0.06], // Rich Red
            [0.42, 0.24, 0.14], // Warm Skin / Orange
            [0.08, 0.36, 0.10], // Foliage Green
            [0.06, 0.16, 0.52], // Sky Blue
            [0.18, 0.18, 0.18], // 18% Middle Gray
        ];

        let mut signatures: Vec<(String, Vec<[f32; 3]>)> = Vec::new();
        for desc in &all {
            let lut = bake_builtin_profile_lut(&desc.id);
            assert_eq!(lut.size, PROFILE_LUT_SIZE);
            assert_eq!(lut.data.len(), PROFILE_LUT_SIZE.pow(3));

            // All LUT texels must be finite and within [0, 1].
            for &[r, g, b] in &lut.data {
                assert!(
                    r.is_finite()
                        && g.is_finite()
                        && b.is_finite()
                        && (0.0..=1.0).contains(&r)
                        && (0.0..=1.0).contains(&g)
                        && (0.0..=1.0).contains(&b),
                    "invalid LUT texel in '{}': [{r}, {g}, {b}]",
                    desc.id
                );
            }

            // Strictly monotonic luminance along the neutral gray ramp.
            let mut prev_luma = -1.0_f32;
            for &g in &gray_ramp {
                let out = lut.sample_trilinear([g, g, g]);
                assert!(out.iter().all(|v| v.is_finite() && *v >= 0.0));
                let luma = 0.2126 * out[0] + 0.7152 * out[1] + 0.0722 * out[2];
                assert!(
                    luma + 1e-5 >= prev_luma,
                    "profile '{}' non-monotonic at gray={g}: {luma} < {prev_luma}",
                    desc.id
                );
                prev_luma = luma;
            }

            // Collect signature across color probes to confirm all non-identity profiles are distinct.
            let sig: Vec<[f32; 3]> = probe_colors
                .iter()
                .map(|&px| lut.sample_trilinear(px))
                .collect();
            if desc.id != "auto" && desc.id != "linear_scientific" {
                assert!(
                    !lut.is_identity(),
                    "profile '{}' should not be identity",
                    desc.id
                );
                signatures.push((desc.id.clone(), sig));
            }
        }

        // Every pair of non-identity profiles must produce visibly distinct color output.
        for i in 0..signatures.len() {
            for j in (i + 1)..signatures.len() {
                let max_diff = signatures[i]
                    .1
                    .iter()
                    .zip(signatures[j].1.iter())
                    .flat_map(|(a, b)| {
                        [
                            (a[0] - b[0]).abs(),
                            (a[1] - b[1]).abs(),
                            (a[2] - b[2]).abs(),
                        ]
                    })
                    .fold(0.0_f32, f32::max);
                assert!(
                    max_diff > 1e-3,
                    "profiles '{}' and '{}' are not distinct (max_diff={max_diff})",
                    signatures[i].0,
                    signatures[j].0
                );
            }
        }

        // Acros Monochrome must produce strictly achromatic output on saturated color patches.
        let acros = bake_builtin_profile_lut("acros_mono");
        for &px in &probe_colors {
            let out = acros.sample_trilinear(px);
            assert!(
                (out[0] - out[1]).abs() < 1e-4 && (out[1] - out[2]).abs() < 1e-4,
                "acros_mono must be strictly monochrome on {px:?}, got {out:?}"
            );
        }

        // Rune ProStandard must preserve OKLab hue (< 1.5° drift) across red, skin, green, and blue,
        // and keep neutral grays strictly achromatic across [0.01, 64.0] (channel spread / luma < 0.005).
        let pro = bake_builtin_profile_lut("pro_standard");
        for &g in &[
            0.01_f32, 0.04, 0.18, 0.50, 1.0, 2.5, 8.0, 16.0, 32.0, 48.0, 64.0,
        ] {
            let exact = eval_pro_standard([g, g, g]);
            let exact_tol = (exact[1] * 1e-5).max(1e-6);
            assert!(
                (exact[0] - exact[1]).abs() < exact_tol && (exact[1] - exact[2]).abs() < exact_tol,
                "pro_standard analytical neutral gray {g} shifted: {exact:?}"
            );
            let out = pro.sample_trilinear([g, g, g]);
            let luma = (0.2126 * out[0] + 0.7152 * out[1] + 0.0722 * out[2]).max(1e-6);
            let spread = out[0].max(out[1]).max(out[2]) - out[0].min(out[1]).min(out[2]);
            assert!(
                spread / luma < 0.005,
                "pro_standard LUT neutral gray {g} spread/luma={} >= 0.005: {out:?}",
                spread / luma
            );
        }
        for &px in &probe_colors[..4] {
            let in_lab = linear_srgb_to_oklab(px);
            let out_rgb = pro.sample_trilinear(px);
            let out_lab = linear_srgb_to_oklab(out_rgb);
            let h_in = in_lab[2].atan2(in_lab[1]).to_degrees().rem_euclid(360.0);
            let h_out = out_lab[2].atan2(out_lab[1]).to_degrees().rem_euclid(360.0);
            let hue_drift = ((h_out - h_in + 540.0).rem_euclid(360.0) - 180.0).abs();
            assert!(
                hue_drift < 1.5,
                "pro_standard OKLab hue drifted {hue_drift:.2}° on {px:?} ({h_in:.1}° -> {h_out:.1}°)"
            );
        }

        // C1 continuity check at diffuse white (L = 1.0) across all 14 profiles' lightness responses.
        let eps = 1e-4_f32;
        let eval_profile_luma = |id: &str, l: f32| -> f32 {
            let lin = l * l * l;
            let rgb = match id {
                "auto" | "linear_scientific" => [lin, lin, lin],
                "pro_standard" => eval_pro_standard([lin, lin, lin]),
                "camera_generic" => eval_camera_generic([lin, lin, lin]),
                "apple_raw" => eval_apple_raw([lin, lin, lin]),
                "portrait_natural" => eval_portrait_natural([lin, lin, lin]),
                "landscape_crisp" => eval_landscape_crisp([lin, lin, lin]),
                "editorial_neutral" => eval_editorial_neutral([lin, lin, lin]),
                "classic_chrome" => eval_classic_chrome([lin, lin, lin]),
                "vivid_velvia" => eval_vivid_velvia([lin, lin, lin]),
                "soft_astia" => eval_soft_astia([lin, lin, lin]),
                "acros_mono" => eval_acros_mono([lin, lin, lin]),
                "warm_portra" => eval_warm_portra([lin, lin, lin]),
                "tungsten_cine" => eval_tungsten_cine([lin, lin, lin]),
                _ => [lin, lin, lin],
            };
            linear_srgb_to_oklab(rgb)[0]
        };
        for desc in &all {
            let l_mid = eval_profile_luma(&desc.id, 1.0);
            let l_lo = eval_profile_luma(&desc.id, 1.0 - eps);
            let l_hi = eval_profile_luma(&desc.id, 1.0 + eps);
            let slope_minus = (l_mid - l_lo) / eps;
            let slope_plus = (l_hi - l_mid) / eps;
            assert!(
                (slope_minus - slope_plus).abs() < 0.02,
                "profile '{}' failed C1 continuity at L=1.0: slope(1-)={slope_minus:.4} vs slope(1+)={slope_plus:.4}",
                desc.id
            );
        }

        // profile_strength == 0.0 and "auto" must both evaluate as exact identity (<= 1e-6).
        let base_recipe = crate::DevelopRecipe::default().compile();
        let zero_strength = crate::DevelopRecipe {
            camera_profile: "vivid_velvia".to_owned(),
            profile_strength: 0.0,
            ..crate::DevelopRecipe::default()
        }
        .compile();
        assert!(!base_recipe.profile_active);
        assert!(!zero_strength.profile_active);
        for &px in &probe_colors {
            let a = base_recipe.evaluate_pixel_scene_linear(px);
            let b = zero_strength.evaluate_pixel_scene_linear(px);
            for ch in 0..3 {
                assert!((a[ch] - px[ch]).abs() <= 1e-6);
                assert!((b[ch] - px[ch]).abs() <= 1e-6);
            }
        }
    }
}
