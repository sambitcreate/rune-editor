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
}

impl CropAspect {
    pub const ALL: [Self; 7] = [
        Self::Free,
        Self::Original,
        Self::Square,
        Self::FourThree,
        Self::ThreeTwo,
        Self::SixteenNine,
        Self::FiveFour,
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

/// Persistent non-destructive Crop, Straighten, Rotation, and Flip geometry.
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
        }
    }
}

impl GeometryParams {
    /// Returns `true` when no pixel-altering geometry transform or crop is active.
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

    /// Maps an oriented-frame UV `(ou, ov)` in `[0, 1]^2` back to source-image UV `(su, sv)`.
    ///
    /// Returns `None` if the rotated point falls outside the source image bounds `[0, 1]^2`.
    /// Matches `oriented_to_source_uv` in `shaders/develop.wgsl` bit-for-bit.
    #[inline]
    pub fn oriented_to_source_uv(
        &self,
        ou: f32,
        ov: f32,
        src_w: u32,
        src_h: u32,
    ) -> Option<[f32; 2]> {
        let [ori_w, ori_h] = self.oriented_size(src_w, src_h);
        let ori_wf = ori_w as f32;
        let ori_hf = ori_h as f32;

        let deg = self.sanitized_straighten_deg();
        let (mut u, mut v) = if deg.abs() > 1e-5 {
            let theta = deg.to_radians();
            let (s, c) = theta.sin_cos();
            let px = (ou - 0.5) * ori_wf;
            let py = (ov - 0.5) * ori_hf;
            let rx = px * c + py * s;
            let ry = -px * s + py * c;
            (rx / ori_wf + 0.5, ry / ori_hf + 0.5)
        } else {
            (ou, ov)
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

        if (0.0..=1.0).contains(&su) && (0.0..=1.0).contains(&sv) {
            Some([su, sv])
        } else {
            None
        }
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
        let [cx, cy, cw, ch] = self.sanitized_crop();
        let ou = cx + out_u * cw;
        let ov = cy + out_v * ch;
        self.oriented_to_source_uv(ou, ov, src_w, src_h)
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
        self.oriented_to_source_uv(ou, ov, src_size[0], src_size[1])
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
}
