//! Persistent non-destructive geometry, crop, straighten, rotation, flip, and composition grids.
//!
//! Coordinate convention (shared identically by `shaders/develop.wgsl`, CPU export, and UI pickers):
//! - Source image UVs `(su, sv)` lie in `[0, 1]^2` with `(0, 0)` top-left and `(1, 1)` bottom-right.
//! - Quarter-turn `rotate_90` (clockwise `0..=3`), `flip_h`/`flip_v`, and `straighten_deg`
//!   (`-45..=+45°` clockwise) define the **oriented frame** `(ori_w, ori_h)` with UVs `(ou, ov)` in `[0, 1]^2`.
//! - `crop = [x, y, w, h]` is normalized in oriented-frame `[0, 1]^2` coordinates.

use serde::{Deserialize, Serialize};

use crate::ViewportCamera;

const fn default_crop_rect() -> [f32; 4] {
    [0.0, 0.0, 1.0, 1.0]
}

const fn default_keystone_amount() -> f32 {
    0.8
}

/// Capture One-style Optical Lens Correction parameters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LensParams {
    /// Radial barrel (<0) / pincushion (>0) lens distortion correction (`-1.0..=1.0`, default `0.0`).
    #[serde(default)]
    pub distortion: f32,
    /// Lateral chromatic aberration (R/B radial dispersion) correction (`-1.0..=1.0`, default `0.0`).
    #[serde(default)]
    pub chromatic_aberration: f32,
    /// High-contrast purple/magenta axial edge fringing suppression (`0.0..=1.0`, default `0.0`).
    #[serde(default)]
    pub purple_fringing: f32,
    /// Optical radial `cos^4` corner light falloff compensation (`0.0..=1.0`, default `0.0`).
    #[serde(default)]
    pub light_falloff: f32,
}

impl LensParams {
    pub fn is_identity(&self) -> bool {
        self.distortion.abs() <= 1e-6
            && self.chromatic_aberration.abs() <= 1e-6
            && self.purple_fringing.abs() <= 1e-6
            && self.light_falloff.abs() <= 1e-6
    }

    pub fn sanitized(self) -> Self {
        let clamp = |v: f32, lo: f32, hi: f32| {
            if v.is_finite() { v.clamp(lo, hi) } else { 0.0 }
        };
        Self {
            distortion: clamp(self.distortion, -1.0, 1.0),
            chromatic_aberration: clamp(self.chromatic_aberration, -1.0, 1.0),
            purple_fringing: clamp(self.purple_fringing, 0.0, 1.0),
            light_falloff: clamp(self.light_falloff, 0.0, 1.0),
        }
    }

    /// Packs `[distortion, chromatic_aberration, purple_fringing, light_falloff]` for GPU uniforms.
    #[inline]
    pub fn to_uniform(&self) -> [f32; 4] {
        let s = self.sanitized();
        [
            s.distortion,
            s.chromatic_aberration,
            s.purple_fringing,
            s.light_falloff,
        ]
    }
}

/// Applies radial lens distortion correction (`distortion` in `-1.0..=1.0`) to a source-sensor UV,
/// matching `apply_lens_distortion_uv` in `shaders/develop.wgsl` bit-for-bit.
#[inline]
pub fn apply_lens_distortion_uv(uv: [f32; 2], distortion: f32, src_w: u32, src_h: u32) -> [f32; 2] {
    let d = if distortion.is_finite() {
        distortion.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    if d.abs() <= 1e-6 {
        return uv;
    }
    let w = src_w.max(1) as f32;
    let h = src_h.max(1) as f32;
    let min_dim = w.min(h);
    let dx = (uv[0] - 0.5) * (w / min_dim);
    let dy = (uv[1] - 0.5) * (h / min_dim);
    let r2 = dx * dx + dy * dy;
    let k1 = d * 0.24;
    let k2 = d * 0.06;
    let s_fit = 1.0 / (1.0 + k1.max(0.0) * 0.5 + k2.max(0.0) * 0.25);
    let scale = (1.0 + k1 * r2 + k2 * r2 * r2) * s_fit;
    [
        0.5 + dx * scale * (min_dim / w),
        0.5 + dy * scale * (min_dim / h),
    ]
}

/// Aspect ratio presets for the Crop tool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CropAspect {
    #[default]
    Free,
    Original,
    Square,
    FourThree,
    ThreeTwo,
    SixteenNine,
    FiveFour,
    FourFive,
}

impl CropAspect {
    pub const ALL: [Self; 8] = [
        Self::Free,
        Self::Original,
        Self::Square,
        Self::FourThree,
        Self::ThreeTwo,
        Self::SixteenNine,
        Self::FiveFour,
        Self::FourFive,
    ];

    /// Numeric width/height ratio (`None` for unconstrained `Free`).
    pub fn ratio(self, original_aspect: f32) -> Option<f32> {
        match self {
            Self::Free => None,
            Self::Original => Some(if original_aspect.is_finite() && original_aspect > 1e-3 {
                original_aspect
            } else {
                1.0
            }),
            Self::Square => Some(1.0),
            Self::FourThree => Some(4.0 / 3.0),
            Self::ThreeTwo => Some(3.0 / 2.0),
            Self::SixteenNine => Some(16.0 / 9.0),
            Self::FiveFour => Some(5.0 / 4.0),
            Self::FourFive => Some(4.0 / 5.0),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Free => "Free",
            Self::Original => "Orig",
            Self::Square => "1:1",
            Self::FourThree => "4:3",
            Self::ThreeTwo => "3:2",
            Self::SixteenNine => "16:9",
            Self::FiveFour => "5:4",
            Self::FourFive => "4:5",
        }
    }

    pub const fn cli_slug(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Original => "original",
            Self::Square => "1:1",
            Self::FourThree => "4:3",
            Self::ThreeTwo => "3:2",
            Self::SixteenNine => "16:9",
            Self::FiveFour => "5:4",
            Self::FourFive => "4:5",
        }
    }

    pub fn from_cli_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "free" => Some(Self::Free),
            "original" | "orig" => Some(Self::Original),
            "square" | "1:1" | "1x1" => Some(Self::Square),
            "four_three" | "4:3" | "4x3" => Some(Self::FourThree),
            "three_two" | "3:2" | "3x2" => Some(Self::ThreeTwo),
            "sixteen_nine" | "16:9" | "16x9" => Some(Self::SixteenNine),
            "five_four" | "5:4" | "5x4" => Some(Self::FiveFour),
            "four_five" | "4:5" | "4x5" => Some(Self::FourFive),
            _ => None,
        }
    }
}

/// Composition guide overlay displayed inside the crop frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CompositionGrid {
    None,
    #[default]
    RuleOfThirds,
    GoldenRatio,
    Diagonal,
}

impl CompositionGrid {
    pub const ALL: [Self; 4] = [
        Self::None,
        Self::RuleOfThirds,
        Self::GoldenRatio,
        Self::Diagonal,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "Off",
            Self::RuleOfThirds => "Thirds",
            Self::GoldenRatio => "Golden",
            Self::Diagonal => "Diag",
        }
    }

    pub const fn cli_slug(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::RuleOfThirds => "thirds",
            Self::GoldenRatio => "golden",
            Self::Diagonal => "diagonal",
        }
    }

    pub fn from_cli_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "off" => Some(Self::None),
            "thirds" | "rule_of_thirds" => Some(Self::RuleOfThirds),
            "golden" | "golden_ratio" => Some(Self::GoldenRatio),
            "diag" | "diagonal" => Some(Self::Diagonal),
            _ => None,
        }
    }

    /// Encodes grid mode as a float for GPU uniforms (`0=None, 1=Thirds, 2=Golden, 3=Diagonal`).
    #[inline]
    pub const fn to_uniform_f32(self) -> f32 {
        match self {
            Self::None => 0.0,
            Self::RuleOfThirds => 1.0,
            Self::GoldenRatio => 2.0,
            Self::Diagonal => 3.0,
        }
    }
}

/// Persistent non-destructive Crop, Straighten, Rotation, Flip, and Keystone Perspective geometry.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeometryParams {
    /// Normalized crop rectangle `[x, y, w, h]` in oriented-frame `[0, 1]` space.
    #[serde(default = "default_crop_rect")]
    pub crop: [f32; 4],
    /// Fine horizon straighten angle in degrees (`-45.0..=45.0`, positive = clockwise).
    #[serde(default)]
    pub straighten_deg: f32,
    /// Quarter-turn clockwise rotations (`0..=3`).
    #[serde(default)]
    pub rotate_90: i8,
    /// Horizontal mirror flip.
    #[serde(default)]
    pub flip_h: bool,
    /// Vertical mirror flip.
    #[serde(default)]
    pub flip_v: bool,
    /// Active crop aspect ratio preset.
    #[serde(default)]
    pub aspect_preset: CropAspect,
    /// Composition grid overlay shown in crop mode.
    #[serde(default)]
    pub grid: CompositionGrid,
    /// Vertical perspective keystone tilt (`-1.0..=1.0`, default `0.0`).
    #[serde(default)]
    pub keystone_vertical: f32,
    /// Horizontal perspective keystone tilt (`-1.0..=1.0`, default `0.0`).
    #[serde(default)]
    pub keystone_horizontal: f32,
    /// Keystone vertical aspect ratio compensation (`-0.5..=1.0`, default `0.0`).
    #[serde(default)]
    pub keystone_aspect: f32,
    /// Keystone correction strength multiplier (`0.0..=1.0`, default `0.8`).
    #[serde(default = "default_keystone_amount")]
    pub keystone_amount: f32,
}

impl Default for GeometryParams {
    fn default() -> Self {
        Self {
            crop: default_crop_rect(),
            straighten_deg: 0.0,
            rotate_90: 0,
            flip_h: false,
            flip_v: false,
            aspect_preset: CropAspect::Free,
            grid: CompositionGrid::RuleOfThirds,
            keystone_vertical: 0.0,
            keystone_horizontal: 0.0,
            keystone_aspect: 0.0,
            keystone_amount: default_keystone_amount(),
        }
    }
}

impl GeometryParams {
    /// Returns `true` when no pixel-altering geometry transform, keystone, or crop is active.
    pub fn is_identity(&self) -> bool {
        let c = self.sanitized_crop();
        c[0].abs() <= 1e-5
            && c[1].abs() <= 1e-5
            && (c[2] - 1.0).abs() <= 1e-5
            && (c[3] - 1.0).abs() <= 1e-5
            && self.sanitized_straighten_deg().abs() <= 1e-4
            && self.quarter_turns() == 0
            && !self.flip_h
            && !self.flip_v
            && self.is_keystone_identity()
    }

    /// Returns `true` when all perspective keystone corrections are inactive.
    #[inline]
    pub fn is_keystone_identity(&self) -> bool {
        let [kv, kh, ka, _] = self.sanitized_keystone();
        kv.abs() <= 1e-6 && kh.abs() <= 1e-6 && ka.abs() <= 1e-6
    }

    /// Returns finite, clamped `[keystone_vertical, keystone_horizontal, keystone_aspect, keystone_amount]`.
    #[inline]
    pub fn sanitized_keystone(&self) -> [f32; 4] {
        let c = |v: f32, def: f32, lo: f32, hi: f32| {
            if v.is_finite() { v.clamp(lo, hi) } else { def }
        };
        [
            c(self.keystone_vertical, 0.0, -1.0, 1.0),
            c(self.keystone_horizontal, 0.0, -1.0, 1.0),
            c(self.keystone_aspect, 0.0, -0.5, 1.0),
            c(self.keystone_amount, default_keystone_amount(), 0.0, 1.0),
        ]
    }

    /// Evaluates the 3×3 perspective homography + auto-scale on an oriented-frame UV,
    /// matching `apply_keystone_uv` in `shaders/develop.wgsl` bit-for-bit.
    #[inline]
    pub fn apply_keystone_uv(&self, ou: f32, ov: f32) -> [f32; 2] {
        let [kv_raw, kh_raw, ka, amount] = self.sanitized_keystone();
        let kv = kv_raw * amount * 0.45;
        let kh = kh_raw * amount * 0.45;
        if kv.abs() <= 1e-6 && kh.abs() <= 1e-6 && ka.abs() <= 1e-6 {
            return [ou, ov];
        }
        let x = ou - 0.5;
        let y = ov - 0.5;
        let inv_a = 1.0 / (1.0 + ka * 0.5);
        let y_a = y * inv_a;
        let w = (1.0 + kv * y_a + kh * x).max(0.15);
        let w_min = (1.0 - kv.abs() * 0.5 * inv_a - kh.abs() * 0.5).max(0.20);
        let auto_scale = w_min / inv_a.max(1.0);
        [0.5 + (x * auto_scale) / w, 0.5 + (y_a * auto_scale) / w]
    }

    /// Returns quarter-turns normalized into `0..=3`.
    #[inline]
    pub fn quarter_turns(&self) -> u8 {
        self.rotate_90.rem_euclid(4) as u8
    }

    /// Returns finite, clamped straighten angle in `-45.0..=45.0` degrees.
    #[inline]
    pub fn sanitized_straighten_deg(&self) -> f32 {
        if self.straighten_deg.is_finite() {
            self.straighten_deg.clamp(-45.0, 45.0)
        } else {
            0.0
        }
    }

    /// Returns a strictly valid crop rect `[x, y, w, h]` with `0 <= x,y <= 0.99`,
    /// `0.01 <= w <= 1 - x`, `0.01 <= h <= 1 - y`.
    #[inline]
    pub fn sanitized_crop(&self) -> [f32; 4] {
        let mut x = if self.crop[0].is_finite() {
            self.crop[0]
        } else {
            0.0
        };
        let mut y = if self.crop[1].is_finite() {
            self.crop[1]
        } else {
            0.0
        };
        let mut w = if self.crop[2].is_finite() {
            self.crop[2]
        } else {
            1.0
        };
        let mut h = if self.crop[3].is_finite() {
            self.crop[3]
        } else {
            1.0
        };
        x = x.clamp(0.0, 0.99);
        y = y.clamp(0.0, 0.99);
        w = w.clamp(0.01, 1.0 - x);
        h = h.clamp(0.01, 1.0 - y);
        [x, y, w, h]
    }

    /// Dimensions of the oriented (post-90°-rotation, pre-crop) frame.
    #[inline]
    pub fn oriented_size(&self, src_w: u32, src_h: u32) -> [u32; 2] {
        let w = src_w.max(1);
        let h = src_h.max(1);
        if self.quarter_turns() % 2 == 1 {
            [h, w]
        } else {
            [w, h]
        }
    }

    /// Dimensions of the final cropped output image in pixels (at least `1x1`).
    #[inline]
    pub fn cropped_output_size(&self, src_w: u32, src_h: u32) -> [u32; 2] {
        let [ori_w, ori_h] = self.oriented_size(src_w, src_h);
        let [_, _, cw, ch] = self.sanitized_crop();
        let out_w = ((ori_w as f32) * cw).round().max(1.0) as u32;
        let out_h = ((ori_h as f32) * ch).round().max(1.0) as u32;
        [out_w, out_h]
    }

    /// Maps an oriented-frame UV `(ou, ov)` in `[0, 1]^2` back to unclamped source-image UV `[su, sv]`,
    /// matching `oriented_to_source_uv` in `shaders/develop.wgsl` bit-for-bit.
    ///
    /// Use this before [`apply_lens_distortion_uv`] so negative radial distortion can pull
    /// edge UVs back inside `[0, 1]^2` before checking source bounds.
    #[inline]
    pub fn oriented_to_source_uv_unclamped(
        &self,
        ou: f32,
        ov: f32,
        src_w: u32,
        src_h: u32,
    ) -> [f32; 2] {
        let [ku, kv] = self.apply_keystone_uv(ou, ov);
        let [ori_w, ori_h] = self.oriented_size(src_w, src_h);
        let ori_wf = ori_w as f32;
        let ori_hf = ori_h as f32;

        let deg = self.sanitized_straighten_deg();
        let (mut u, mut v) = if deg.abs() > 1e-5 {
            let theta = deg.to_radians();
            let (s, c) = theta.sin_cos();
            let px = (ku - 0.5) * ori_wf;
            let py = (kv - 0.5) * ori_hf;
            let rx = px * c + py * s;
            let ry = -px * s + py * c;
            (rx / ori_wf + 0.5, ry / ori_hf + 0.5)
        } else {
            (ku, kv)
        };

        if self.flip_h {
            u = 1.0 - u;
        }
        if self.flip_v {
            v = 1.0 - v;
        }

        let (su, sv) = match self.quarter_turns() {
            1 => (v, 1.0 - u),
            2 => (1.0 - u, 1.0 - v),
            3 => (1.0 - v, u),
            _ => (u, v),
        };
        [su, sv]
    }

    /// Maps an oriented-frame UV `(ou, ov)` in `[0, 1]^2` back to source-image UV `(su, sv)`.
    ///
    /// Returns `None` if the rotated point falls outside the source image bounds `[0, 1]^2`.
    #[inline]
    pub fn oriented_to_source_uv(
        &self,
        ou: f32,
        ov: f32,
        src_w: u32,
        src_h: u32,
    ) -> Option<[f32; 2]> {
        let [su, sv] = self.oriented_to_source_uv_unclamped(ou, ov, src_w, src_h);
        if (0.0..=1.0).contains(&su) && (0.0..=1.0).contains(&sv) {
            Some([su, sv])
        } else {
            None
        }
    }

    /// Maps a cropped output UV `(out_u, out_v)` in `[0, 1]^2` directly to unclamped source-image UV.
    #[inline]
    pub fn cropped_to_source_uv_unclamped(
        &self,
        out_u: f32,
        out_v: f32,
        src_w: u32,
        src_h: u32,
    ) -> [f32; 2] {
        let [cx, cy, cw, ch] = self.sanitized_crop();
        let ou = cx + out_u * cw;
        let ov = cy + out_v * ch;
        self.oriented_to_source_uv_unclamped(ou, ov, src_w, src_h)
    }

    /// Maps a cropped output UV `(out_u, out_v)` in `[0, 1]^2` directly to source-image UV.
    ///
    /// Used by full-resolution export when `respect_crop` is enabled so exported pixels match
    /// `shaders/develop.wgsl` identically.
    #[inline]
    pub fn cropped_to_source_uv(
        &self,
        out_u: f32,
        out_v: f32,
        src_w: u32,
        src_h: u32,
    ) -> Option<[f32; 2]> {
        let [su, sv] = self.cropped_to_source_uv_unclamped(out_u, out_v, src_w, src_h);
        if (0.0..=1.0).contains(&su) && (0.0..=1.0).contains(&sv) {
            Some([su, sv])
        } else {
            None
        }
    }

    /// Projects a viewport slot UV (`[0, 1]^2` inside `slot_size` device pixels) into the
    /// active frame's normalized `[frame_u, frame_v]` space (unclamped).
    fn viewport_to_frame_uv(
        &self,
        slot_uv: [f32; 2],
        slot_size: [f32; 2],
        src_size: [u32; 2],
        camera: &ViewportCamera,
    ) -> [f32; 2] {
        let slot_w = slot_size[0].max(1.0);
        let slot_h = slot_size[1].max(1.0);
        let [ori_w, ori_h] = self.oriented_size(src_size[0], src_size[1]);
        let [_, _, cw, ch] = self.sanitized_crop();

        let (frame_w, frame_h) = if camera.crop_mode {
            (ori_w as f32, ori_h as f32)
        } else {
            ((ori_w as f32 * cw).max(1.0), (ori_h as f32 * ch).max(1.0))
        };

        let pad = 28.0_f32;
        let avail_w = (slot_w - pad * 2.0).max(1.0);
        let avail_h = (slot_h - pad * 2.0).max(1.0);
        let fit_scale = (avail_w / frame_w).min(avail_h / frame_h);
        let fitted_w = frame_w * fit_scale;
        let fitted_h = frame_h * fit_scale;

        let zoom = camera.zoom.max(0.05);
        let pan_x = camera.pan[0] * slot_w * 0.5;
        let pan_y = camera.pan[1] * slot_h * 0.5;
        let (sn, cs) = (-camera.rotation_deg.to_radians()).sin_cos();

        let px = (slot_uv[0] - 0.5) * slot_w - pan_x;
        let py = (slot_uv[1] - 0.5) * slot_h - pan_y;
        let rx = px * cs - py * sn;
        let ry = px * sn + py * cs;

        [rx / (fitted_w * zoom) + 0.5, ry / (fitted_h * zoom) + 0.5]
    }

    /// Maps a viewport slot UV (`[0, 1]^2` inside `slot_size` device pixels) into unclamped
    /// oriented-frame UV `[ou, ov]`, matching `fs_main` in `shaders/develop.wgsl`.
    ///
    /// Use this for interactive crop dragging (including drags extending outside `[0, 1]`).
    pub fn viewport_to_oriented_uv(
        &self,
        slot_uv: [f32; 2],
        slot_size: [f32; 2],
        src_size: [u32; 2],
        camera: &ViewportCamera,
    ) -> [f32; 2] {
        let [frame_u, frame_v] = self.viewport_to_frame_uv(slot_uv, slot_size, src_size, camera);
        if camera.crop_mode {
            [frame_u, frame_v]
        } else {
            let [cx, cy, cw, ch] = self.sanitized_crop();
            [cx + frame_u * cw, cy + frame_v * ch]
        }
    }

    /// Maps a viewport slot UV (`[0, 1]^2` inside the canvas slot of size `slot_w × slot_h`)
    /// to source-image UV `[su, sv]`, matching `fs_main` in `shaders/develop.wgsl` exactly.
    ///
    /// Use this for live cursor Color Readouts and the White Balance Eyedropper.
    pub fn viewport_to_source_uv(
        &self,
        slot_uv: [f32; 2],
        slot_size: [f32; 2],
        src_size: [u32; 2],
        camera: &ViewportCamera,
    ) -> Option<[f32; 2]> {
        self.viewport_to_source_uv_with_lens(slot_uv, slot_size, src_size, camera, 0.0)
    }

    /// Maps a viewport slot UV (`[0, 1]^2` inside `slot_size`) to lens-distortion-adjusted
    /// source-image UV `[lu, lv]`, checking `[0, 1]^2` bounds *after* [`apply_lens_distortion_uv`]
    /// to match `fs_main` in `shaders/develop.wgsl` exactly.
    pub fn viewport_to_source_uv_with_lens(
        &self,
        slot_uv: [f32; 2],
        slot_size: [f32; 2],
        src_size: [u32; 2],
        camera: &ViewportCamera,
        distortion: f32,
    ) -> Option<[f32; 2]> {
        let [frame_u, frame_v] = self.viewport_to_frame_uv(slot_uv, slot_size, src_size, camera);
        if !(0.0..=1.0).contains(&frame_u) || !(0.0..=1.0).contains(&frame_v) {
            return None;
        }
        let [ou, ov] = if camera.crop_mode {
            [frame_u, frame_v]
        } else {
            let [cx, cy, cw, ch] = self.sanitized_crop();
            [cx + frame_u * cw, cy + frame_v * ch]
        };
        let raw_uv = self.oriented_to_source_uv_unclamped(ou, ov, src_size[0], src_size[1]);
        let [lu, lv] = apply_lens_distortion_uv(raw_uv, distortion, src_size[0], src_size[1]);
        if (0.0..=1.0).contains(&lu) && (0.0..=1.0).contains(&lv) {
            Some([lu, lv])
        } else {
            None
        }
    }

    /// Snaps `self.crop` to the given `CropAspect` centered around the current crop center,
    /// fitting cleanly inside `[0, 1]^2`.
    pub fn apply_aspect_preset(&mut self, preset: CropAspect, src_w: u32, src_h: u32) {
        self.aspect_preset = preset;
        let [ori_w, ori_h] = self.oriented_size(src_w, src_h);
        let ori_aspect = ori_w as f32 / ori_h as f32;
        let Some(target_ratio) = preset.ratio(ori_aspect) else {
            return;
        };

        // Target ratio in normalized UV coordinates: (w * ori_w) / (h * ori_h) = target_ratio
        // => w / h = target_ratio / ori_aspect.
        let norm_ratio = target_ratio / ori_aspect;
        let [cx, cy, cw, ch] = self.sanitized_crop();
        let center_x = cx + cw * 0.5;
        let center_y = cy + ch * 0.5;

        let (mut nw, mut nh) = if cw / ch > norm_ratio {
            (ch * norm_ratio, ch)
        } else {
            (cw, cw / norm_ratio)
        };
        if nw > 1.0 {
            nh /= nw;
            nw = 1.0;
        }
        if nh > 1.0 {
            nw /= nh;
            nh = 1.0;
        }
        nw = nw.clamp(0.01, 1.0);
        nh = nh.clamp(0.01, 1.0);
        let nx = (center_x - nw * 0.5).clamp(0.0, 1.0 - nw);
        let ny = (center_y - nh * 0.5).clamp(0.0, 1.0 - nh);
        self.crop = [nx, ny, nw, nh];
    }

    /// Resets crop, straighten, rotation, and flip back to full uncropped identity.
    pub fn reset(&mut self) {
        let grid = self.grid;
        *self = Self {
            grid,
            ..Self::default()
        };
    }

    /// Packs `[straighten_rad, quarter_turns, flip_h, flip_v]` for GPU uniforms.
    #[inline]
    pub fn to_xform_uniform(&self) -> [f32; 4] {
        [
            self.sanitized_straighten_deg().to_radians(),
            self.quarter_turns() as f32,
            if self.flip_h { 1.0 } else { 0.0 },
            if self.flip_v { 1.0 } else { 0.0 },
        ]
    }

    /// Packs `[keystone_vertical, keystone_horizontal, keystone_aspect, keystone_amount]`
    /// for 16-byte aligned GPU uniforms.
    #[inline]
    pub fn to_keystone_uniform(&self) -> [f32; 4] {
        self.sanitized_keystone()
    }
}
