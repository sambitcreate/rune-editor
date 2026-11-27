//! Monotonic cubic Hermite (Fritsch-Carlson) RGB & Luma tone curves
//! and 256-entry LUT baking for CPU/GPU parity.

use half::f16;
use serde::{Deserialize, Serialize};

/// Number of entries in the 1D tone curve LUT texture (`256x1` `Rgba16Float`).
pub const CURVE_LUT_SIZE: usize = 256;

/// Default identity tone curve control points `[[0, 0], [1, 1]]`.
pub fn default_curve_points() -> Vec<[f32; 2]> {
    vec![[0.0, 0.0], [1.0, 1.0]]
}

/// Non-destructive RGB Combined, Master Luma, and per-channel R/G/B tone curves.
///
/// Control points are `[x, y]` pairs in `[0.0, 1.0]` perceptual tone space.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToneCurves {
    #[serde(default = "default_curve_points")]
    pub rgb: Vec<[f32; 2]>,
    #[serde(default = "default_curve_points")]
    pub luma: Vec<[f32; 2]>,
    #[serde(default = "default_curve_points")]
    pub red: Vec<[f32; 2]>,
    #[serde(default = "default_curve_points")]
    pub green: Vec<[f32; 2]>,
    #[serde(default = "default_curve_points")]
    pub blue: Vec<[f32; 2]>,
}

impl Default for ToneCurves {
    fn default() -> Self {
        Self {
            rgb: default_curve_points(),
            luma: default_curve_points(),
            red: default_curve_points(),
            green: default_curve_points(),
            blue: default_curve_points(),
        }
    }
}

impl ToneCurves {
    /// Returns `true` when all five channels represent an identity tone curve.
    pub fn is_identity(&self) -> bool {
        channel_is_identity(&self.rgb)
            && channel_is_identity(&self.luma)
            && channel_is_identity(&self.red)
            && channel_is_identity(&self.green)
            && channel_is_identity(&self.blue)
    }

    /// Evaluates a single channel's control points at `x` in `[0.0, 1.0]`
    /// using a monotonic cubic Hermite (Fritsch-Carlson) spline.
    pub fn evaluate_channel(points: &[[f32; 2]], x: f32) -> f32 {
        let spline = MonotonicSpline::from_points(points);
        spline.eval(x)
    }

    /// Bakes all 5 tone curves into a 256-entry `f32` RGBA LUT where each entry is
    /// `[rgb(red(x)), rgb(green(x)), rgb(blue(x)), luma(x)]`.
    pub fn bake_curve_lut_f32(&self) -> [[f32; 4]; CURVE_LUT_SIZE] {
        let rgb_spline = MonotonicSpline::from_points(&self.rgb);
        let r_spline = MonotonicSpline::from_points(&self.red);
        let g_spline = MonotonicSpline::from_points(&self.green);
        let b_spline = MonotonicSpline::from_points(&self.blue);
        let l_spline = MonotonicSpline::from_points(&self.luma);

        let mut lut = [[0.0_f32; 4]; CURVE_LUT_SIZE];
        let denom = (CURVE_LUT_SIZE - 1) as f32;
        for (i, entry) in lut.iter_mut().enumerate() {
            let x = i as f32 / denom;
            *entry = [
                rgb_spline.eval(r_spline.eval(x)),
                rgb_spline.eval(g_spline.eval(x)),
                rgb_spline.eval(b_spline.eval(x)),
                l_spline.eval(x),
            ];
        }
        lut
    }

    /// Bakes all 4 tone curves into a 256-entry `f16` RGBA LUT (`[red, green, blue, luma]`)
    /// ready for direct upload into a `256x1` `wgpu::TextureFormat::Rgba16Float` texture.
    pub fn bake_curve_lut_f16(&self) -> [[f16; 4]; CURVE_LUT_SIZE] {
        let lut_f32 = self.bake_curve_lut_f32();
        let mut lut_f16 = [[f16::ZERO; 4]; CURVE_LUT_SIZE];
        for (dst, src) in lut_f16.iter_mut().zip(lut_f32.iter()) {
            *dst = [
                f16::from_f32(src[0]),
                f16::from_f32(src[1]),
                f16::from_f32(src[2]),
                f16::from_f32(src[3]),
            ];
        }
        lut_f16
    }

    /// Returns a 256-entry identity `f16` RGBA LUT (`[x, x, x, x]`).
    pub fn identity_lut_f16() -> [[f16; 4]; CURVE_LUT_SIZE] {
        let mut lut = [[f16::ZERO; 4]; CURVE_LUT_SIZE];
        let denom = (CURVE_LUT_SIZE - 1) as f32;
        for (i, entry) in lut.iter_mut().enumerate() {
            let v = f16::from_f32(i as f32 / denom);
            *entry = [v, v, v, v];
        }
        lut
    }
}

fn channel_is_identity(points: &[[f32; 2]]) -> bool {
    if points.is_empty() {
        return true;
    }
    if points.len() == 2
        && (points[0][0] - 0.0).abs() < 1e-5
        && (points[0][1] - 0.0).abs() < 1e-5
        && (points[1][0] - 1.0).abs() < 1e-5
        && (points[1][1] - 1.0).abs() < 1e-5
    {
        return true;
    }
    let spline = MonotonicSpline::from_points(points);
    for i in 0..=16 {
        let x = i as f32 / 16.0;
        if (spline.eval(x) - x).abs() > 1e-4 {
            return false;
        }
    }
    true
}

/// Precomputed Fritsch-Carlson monotonic cubic Hermite spline.
pub(crate) struct MonotonicSpline {
    xs: Vec<f32>,
    ys: Vec<f32>,
    ms: Vec<f32>,
}

impl MonotonicSpline {
    pub(crate) fn from_points(raw_points: &[[f32; 2]]) -> Self {
        let mut pts: Vec<[f32; 2]> = raw_points
            .iter()
            .filter(|p| p[0].is_finite() && p[1].is_finite())
            .map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)])
            .collect();

        if pts.is_empty() {
            return Self {
                xs: vec![0.0, 1.0],
                ys: vec![0.0, 1.0],
                ms: vec![1.0, 1.0],
            };
        }

        pts.sort_by(|a, b| a[0].total_cmp(&b[0]));

        // Deduplicate points with nearly identical X coordinates.
        let mut xs: Vec<f32> = Vec::with_capacity(pts.len());
        let mut ys: Vec<f32> = Vec::with_capacity(pts.len());
        for [x, y] in pts {
            if let Some(&last_x) = xs.last()
                && (x - last_x).abs() < 1e-5
            {
                if let Some(last_y) = ys.last_mut() {
                    *last_y = y;
                }
                continue;
            }
            xs.push(x);
            ys.push(y);
        }

        let n = xs.len();
        if n == 1 {
            return Self {
                xs,
                ys,
                ms: vec![0.0],
            };
        }

        // Secant slopes d_k = (y_{k+1} - y_k) / (x_{k+1} - x_k)
        let mut d = vec![0.0_f32; n - 1];
        for k in 0..n - 1 {
            let h = xs[k + 1] - xs[k];
            d[k] = if h > 1e-6 {
                (ys[k + 1] - ys[k]) / h
            } else {
                0.0
            };
        }

        // Initial tangents m_k
        let mut m = vec![0.0_f32; n];
        m[0] = d[0];
        m[n - 1] = d[n - 2];
        for k in 1..n - 1 {
            if d[k - 1] * d[k] <= 0.0 {
                m[k] = 0.0;
            } else {
                // Weighted harmonic mean preserves monotonicity across uneven knot spacing.
                let h0 = xs[k] - xs[k - 1];
                let h1 = xs[k + 1] - xs[k];
                let w1 = 2.0 * h1 + h0;
                let w2 = h1 + 2.0 * h0;
                m[k] = (w1 + w2) / (w1 / d[k - 1] + w2 / d[k]);
            }
        }

        // Fritsch-Carlson monotonicity constraint (alpha^2 + beta^2 <= 9)
        for k in 0..n - 1 {
            let dk = d[k];
            if dk.abs() < 1e-6 {
                m[k] = 0.0;
                m[k + 1] = 0.0;
                continue;
            }
            let alpha = (m[k] / dk).max(0.0);
            let beta = (m[k + 1] / dk).max(0.0);
            let mag_sq = alpha * alpha + beta * beta;
            if mag_sq > 9.0 {
                let tau = 3.0 / mag_sq.sqrt();
                m[k] = tau * alpha * dk;
                m[k + 1] = tau * beta * dk;
            } else {
                m[k] = alpha * dk;
                m[k + 1] = beta * dk;
            }
        }

        Self { xs, ys, ms: m }
    }

    #[inline]
    pub(crate) fn eval(&self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        let n = self.xs.len();
        if n == 1 {
            return self.ys[0].clamp(0.0, 1.0);
        }
        if x <= self.xs[0] {
            return self.ys[0].clamp(0.0, 1.0);
        }
        if x >= self.xs[n - 1] {
            return self.ys[n - 1].clamp(0.0, 1.0);
        }

        // Locate segment k such that xs[k] <= x <= xs[k + 1]
        let mut lo = 0usize;
        let mut hi = n - 1;
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if self.xs[mid] <= x {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let k = lo;
        let h = self.xs[k + 1] - self.xs[k];
        if h <= 1e-6 {
            return self.ys[k].clamp(0.0, 1.0);
        }
        let t = ((x - self.xs[k]) / h).clamp(0.0, 1.0);
        let t2 = t * t;
        let t3 = t2 * t;

        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;

        let y = h00 * self.ys[k]
            + h10 * h * self.ms[k]
            + h01 * self.ys[k + 1]
            + h11 * h * self.ms[k + 1];
        y.clamp(0.0, 1.0)
    }
}

/// Samples a baked 256-entry `f32` LUT using exact linear interpolation matching
/// GPU `FilterMode::Linear` on texel centers `u = (0.5 + x * 255.0) / 256.0`.
#[inline]
pub fn sample_curve_lut(lut: &[[f32; 4]; CURVE_LUT_SIZE], x: f32) -> [f32; 4] {
    let clamped = x.clamp(0.0, 1.0) * (CURVE_LUT_SIZE - 1) as f32;
    let i0 = (clamped.floor() as usize).min(CURVE_LUT_SIZE - 1);
    let i1 = (i0 + 1).min(CURVE_LUT_SIZE - 1);
    let frac = clamped - i0 as f32;
    let a = lut[i0];
    let b = lut[i1];
    [
        a[0] + (b[0] - a[0]) * frac,
        a[1] + (b[1] - a[1]) * frac,
        a[2] + (b[2] - a[2]) * frac,
        a[3] + (b[3] - a[3]) * frac,
    ]
}
