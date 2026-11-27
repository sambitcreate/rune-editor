//! Capture One-style Adjustment Layers, Composable Masks (`Add`/`Subtract`/`Intersect`),
//! Linear/Radial/Luma/Brush/Raster masks, Spot Heal & Clone, and unified `2048×1024` `R8Unorm`
//! mask atlas baking with exact GPU↔CPU parity.

use serde::{Deserialize, Serialize};

use crate::CoreError;

/// Maximum number of simultaneous adjustment layers evaluated in the single-pass GPU shader.
pub const MAX_ADJUSTMENT_LAYERS: usize = 8;

/// Maximum number of composable mask items stored per adjustment layer.
pub const MAX_MASKS_PER_LAYER: usize = 4;

/// Resolution (`512×512`) of each layer's baked spatial mask tile inside the `4×2` atlas.
pub const LAYER_MASK_TILE_SIZE: usize = 512;

/// Width in pixels of the `4×2` `R8Unorm` layer mask atlas (`2048`).
pub const LAYER_MASK_ATLAS_WIDTH: u32 = (LAYER_MASK_TILE_SIZE * 4) as u32;

/// Height in pixels of the `4×2` `R8Unorm` layer mask atlas (`1024`).
pub const LAYER_MASK_ATLAS_HEIGHT: u32 = (LAYER_MASK_TILE_SIZE * 2) as u32;

/// Maximum number of Spot Heal / Clone stamps evaluated in Stage 0.
pub const MAX_HEAL_SPOTS: usize = 16;

/// Maximum number of brush/eraser dabs retained per `MaskShape::BrushStrokes` item
/// to bound CPU mask baking work against untrusted sidecar or MCP payloads.
pub const MAX_BRUSH_DABS_PER_MASK: usize = 1024;

/// Maximum width or height accepted for [`MaskShape::RasterMask`] (`1024×1024`) to prevent
/// OOM denial-of-service from untrusted sidecars while supporting legacy `256×256`, native
/// `512×512`, and high-resolution `1024×1024` masks.
pub const MAX_RASTER_MASK_DIM: u32 = 1024;

/// Maximum base64 byte length accepted by [`MaskShape::decode_raster_u8`] (`1024×1024×2` bytes).
const MAX_RASTER_MASK_B64_LEN: usize =
    (MAX_RASTER_MASK_DIM as usize) * (MAX_RASTER_MASK_DIM as usize) * 2;

const fn default_true() -> bool {
    true
}

const fn default_one() -> f32 {
    1.0
}

const fn default_heal_radius() -> f32 {
    0.03
}

const fn default_heal_feather() -> f32 {
    0.4
}

const fn default_brush_radius() -> f32 {
    0.06
}

const fn default_brush_hardness() -> f32 {
    0.4
}

/// Boolean/blended composition operator when stacking multiple masks inside one layer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskCombineOp {
    #[default]
    Add,
    Subtract,
    Intersect,
}

impl MaskCombineOp {
    pub const ALL: [Self; 3] = [Self::Add, Self::Subtract, Self::Intersect];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Add => "Add",
            Self::Subtract => "Sub",
            Self::Intersect => "Int",
        }
    }

    pub const fn cli_slug(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Subtract => "subtract",
            Self::Intersect => "intersect",
        }
    }

    pub fn from_cli_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "add" | "union" => Some(Self::Add),
            "subtract" | "sub" => Some(Self::Subtract),
            "intersect" | "int" => Some(Self::Intersect),
            _ => None,
        }
    }

    #[inline]
    pub const fn to_uniform_f32(self) -> f32 {
        match self {
            Self::Add => 0.0,
            Self::Subtract => 1.0,
            Self::Intersect => 2.0,
        }
    }

    #[inline]
    pub fn combine(self, base: f32, next: f32) -> f32 {
        let a = base.clamp(0.0, 1.0);
        let b = next.clamp(0.0, 1.0);
        match self {
            Self::Add => a.max(b),
            Self::Subtract => (a - b).clamp(0.0, 1.0),
            Self::Intersect => (a * b).clamp(0.0, 1.0),
        }
    }
}

/// Single soft/hard circular brush or eraser dab in normalized `[0, 1]^2` source image UV space.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrushDab {
    pub x: f32,
    pub y: f32,
    #[serde(default = "default_brush_radius")]
    pub radius: f32,
    #[serde(default = "default_brush_hardness")]
    pub hardness: f32,
    #[serde(default = "default_one")]
    pub opacity: f32,
    pub erase: bool,
}

impl Default for BrushDab {
    fn default() -> Self {
        Self {
            x: 0.5,
            y: 0.5,
            radius: default_brush_radius(),
            hardness: default_brush_hardness(),
            opacity: 1.0,
            erase: false,
        }
    }
}

impl BrushDab {
    pub fn sanitized(self) -> Self {
        let f = |v: f32, def: f32, lo: f32, hi: f32| {
            if v.is_finite() { v.clamp(lo, hi) } else { def }
        };
        Self {
            x: f(self.x, 0.5, 0.0, 1.0),
            y: f(self.y, 0.5, 0.0, 1.0),
            radius: f(self.radius, default_brush_radius(), 0.002, 0.5),
            hardness: f(self.hardness, default_brush_hardness(), 0.0, 1.0),
            opacity: f(self.opacity, 1.0, 0.0, 1.0),
            erase: self.erase,
        }
    }
}

/// Composable mask primitive attached to an [`AdjustmentLayer`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MaskShape {
    /// Uniform `1.0` mask across the entire image.
    Filled,
    /// Directional gradient transitioning smoothly from `start` (`1.0`) to `end` (`0.0`).
    LinearGradient {
        start: [f32; 2],
        end: [f32; 2],
        #[serde(default = "default_one")]
        feather: f32,
    },
    /// Elliptical radial mask centered at `center` with semi-axes `radius`, rotated by `angle_deg`.
    RadialGradient {
        center: [f32; 2],
        radius: [f32; 2],
        #[serde(default)]
        angle_deg: f32,
        #[serde(default = "default_heal_feather")]
        feather: f32,
    },
    /// Live scene-luminance range mask evaluated per-pixel identically on GPU and CPU.
    LumaRange {
        low: f32,
        high: f32,
        #[serde(default)]
        low_falloff: f32,
        #[serde(default)]
        high_falloff: f32,
    },
    /// Ordered list of circular brush & eraser dabs.
    BrushStrokes {
        #[serde(default)]
        strokes: Vec<BrushDab>,
    },
    /// Compact R8 grayscale raster mask (e.g. AI Subject / AI Background) encoded as base64.
    RasterMask {
        width: u32,
        height: u32,
        data_b64: String,
    },
}

impl MaskShape {
    /// Human-readable short badge for UI & CLI summaries.
    pub const fn kind_label(&self) -> &'static str {
        match self {
            Self::Filled => "Filled",
            Self::LinearGradient { .. } => "Linear",
            Self::RadialGradient { .. } => "Radial",
            Self::LumaRange { .. } => "Luma",
            Self::BrushStrokes { .. } => "Brush",
            Self::RasterMask { .. } => "AI Mask",
        }
    }

    /// Encodes an `R8` (`width * height` bytes) mask buffer into a compact [`MaskShape::RasterMask`].
    pub fn from_raster_u8(width: u32, height: u32, bytes: &[u8]) -> Self {
        let w = width.clamp(1, MAX_RASTER_MASK_DIM);
        let h = height.clamp(1, MAX_RASTER_MASK_DIM);
        let expected = (w as usize) * (h as usize);
        let mut buf = vec![0u8; expected];
        let copy_len = expected.min(bytes.len());
        buf[..copy_len].copy_from_slice(&bytes[..copy_len]);
        Self::RasterMask {
            width: w,
            height: h,
            data_b64: encode_base64(&buf),
        }
    }

    /// Decodes a [`MaskShape::RasterMask`] back into `(width, height, bytes)`.
    pub fn decode_raster_u8(&self) -> Option<(u32, u32, Vec<u8>)> {
        let Self::RasterMask {
            width,
            height,
            data_b64,
        } = self
        else {
            return None;
        };
        if *width == 0
            || *height == 0
            || *width > MAX_RASTER_MASK_DIM
            || *height > MAX_RASTER_MASK_DIM
            || data_b64.len() > MAX_RASTER_MASK_B64_LEN
        {
            return None;
        }
        let expected = (*width as usize).checked_mul(*height as usize)?;
        let mut raw = decode_base64(data_b64)?;
        if raw.len() < expected {
            raw.resize(expected, 0);
        } else if raw.len() > expected {
            raw.truncate(expected);
        }
        Some((*width, *height, raw))
    }

    pub fn sanitized(&self) -> Self {
        let c01 = |v: f32, def: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                def
            }
        };
        let cf = |v: f32, def: f32, lo: f32, hi: f32| {
            if v.is_finite() { v.clamp(lo, hi) } else { def }
        };
        match self {
            Self::Filled => Self::Filled,
            Self::LinearGradient {
                start,
                end,
                feather,
            } => Self::LinearGradient {
                start: [c01(start[0], 0.5), c01(start[1], 0.0)],
                end: [c01(end[0], 0.5), c01(end[1], 0.6)],
                feather: c01(*feather, 0.5),
            },
            Self::RadialGradient {
                center,
                radius,
                angle_deg,
                feather,
            } => Self::RadialGradient {
                center: [c01(center[0], 0.5), c01(center[1], 0.5)],
                radius: [cf(radius[0], 0.3, 0.01, 1.5), cf(radius[1], 0.3, 0.01, 1.5)],
                angle_deg: cf(*angle_deg, 0.0, -180.0, 180.0),
                feather: c01(*feather, 0.4),
            },
            Self::LumaRange {
                low,
                high,
                low_falloff,
                high_falloff,
            } => {
                let lo = c01(*low, 0.0);
                let hi = c01(*high, 1.0).max(lo);
                Self::LumaRange {
                    low: lo,
                    high: hi,
                    low_falloff: c01(*low_falloff, 0.1),
                    high_falloff: c01(*high_falloff, 0.1),
                }
            }
            Self::BrushStrokes { strokes } => Self::BrushStrokes {
                strokes: strokes
                    .iter()
                    .take(MAX_BRUSH_DABS_PER_MASK)
                    .map(|d| d.sanitized())
                    .collect(),
            },
            Self::RasterMask {
                width,
                height,
                data_b64,
            } => Self::RasterMask {
                width: (*width).clamp(1, MAX_RASTER_MASK_DIM),
                height: (*height).clamp(1, MAX_RASTER_MASK_DIM),
                data_b64: if data_b64.len() <= MAX_RASTER_MASK_B64_LEN {
                    data_b64.clone()
                } else {
                    String::new()
                },
            },
        }
    }
}

/// A single composable mask entry inside an [`AdjustmentLayer`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerMaskItem {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub op: MaskCombineOp,
    #[serde(default)]
    pub inverted: bool,
    pub shape: MaskShape,
}

impl LayerMaskItem {
    pub fn new(shape: MaskShape) -> Self {
        Self {
            enabled: true,
            op: MaskCombineOp::Add,
            inverted: false,
            shape: shape.sanitized(),
        }
    }

    pub fn sanitized(&self) -> Self {
        Self {
            enabled: self.enabled,
            op: self.op,
            inverted: self.inverted,
            shape: self.shape.sanitized(),
        }
    }
}

/// Local photographic adjustments applied through an [`AdjustmentLayer`]'s mask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalAdjustments {
    /// Local exposure compensation in EV (`-4.0..=4.0`).
    pub exposure: f32,
    /// Local contrast around 18% middle gray (`-1.0..=1.0`).
    pub contrast: f32,
    /// Local midtone brightness lift/dip (`-1.0..=1.0`).
    pub brightness: f32,
    /// Local highlight recovery/boost (`-1.0..=1.0`).
    pub highlights: f32,
    /// Local shadow lift/crush (`-1.0..=1.0`).
    pub shadows: f32,
    /// Local white balance temperature shift (`-1.0..=1.0`).
    pub temperature: f32,
    /// Local white balance tint shift (`-1.0..=1.0`).
    pub tint: f32,
    /// Local color saturation (`-1.0..=1.0`).
    pub saturation: f32,
    /// Local midtone clarity (`-1.0..=1.0`).
    pub clarity: f32,
    /// Local atmospheric dehaze (`-1.0..=1.0`).
    pub dehaze: f32,
    /// Local unsharp sharpening (`0.0..=1.0`).
    pub sharpen: f32,
}

impl LocalAdjustments {
    pub fn is_neutral(&self) -> bool {
        self.exposure.abs() <= 1e-6
            && self.contrast.abs() <= 1e-6
            && self.brightness.abs() <= 1e-6
            && self.highlights.abs() <= 1e-6
            && self.shadows.abs() <= 1e-6
            && self.temperature.abs() <= 1e-6
            && self.tint.abs() <= 1e-6
            && self.saturation.abs() <= 1e-6
            && self.clarity.abs() <= 1e-6
            && self.dehaze.abs() <= 1e-6
            && self.sharpen.abs() <= 1e-6
    }

    pub fn sanitized(self) -> Self {
        let cf = |v: f32, lo: f32, hi: f32| {
            if v.is_finite() { v.clamp(lo, hi) } else { 0.0 }
        };
        Self {
            exposure: cf(self.exposure, -4.0, 4.0),
            contrast: cf(self.contrast, -1.0, 1.0),
            brightness: cf(self.brightness, -1.0, 1.0),
            highlights: cf(self.highlights, -1.0, 1.0),
            shadows: cf(self.shadows, -1.0, 1.0),
            temperature: cf(self.temperature, -1.0, 1.0),
            tint: cf(self.tint, -1.0, 1.0),
            saturation: cf(self.saturation, -1.0, 1.0),
            clarity: cf(self.clarity, -1.0, 1.0),
            dehaze: cf(self.dehaze, -1.0, 1.0),
            sharpen: cf(self.sharpen, 0.0, 1.0),
        }
    }

    /// Sets a local adjustment scalar by canonical name, validating finite bounds.
    pub fn set_by_name(&mut self, key: &str, value: f32) -> Result<(), CoreError> {
        if !value.is_finite() {
            return Err(CoreError::Validation(format!(
                "layer adjustment '{key}' must be finite"
            )));
        }
        let check = |lo: f32, hi: f32| -> Result<f32, CoreError> {
            if value < lo || value > hi {
                Err(CoreError::Validation(format!(
                    "layer adjustment '{key}' must be within {lo}..={hi} (got {value})"
                )))
            } else {
                Ok(value)
            }
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "exposure" => self.exposure = check(-4.0, 4.0)?,
            "contrast" => self.contrast = check(-1.0, 1.0)?,
            "brightness" => self.brightness = check(-1.0, 1.0)?,
            "highlights" => self.highlights = check(-1.0, 1.0)?,
            "shadows" => self.shadows = check(-1.0, 1.0)?,
            "temperature" | "temp" => self.temperature = check(-1.0, 1.0)?,
            "tint" => self.tint = check(-1.0, 1.0)?,
            "saturation" | "sat" => self.saturation = check(-1.0, 1.0)?,
            "clarity" => self.clarity = check(-1.0, 1.0)?,
            "dehaze" => self.dehaze = check(-1.0, 1.0)?,
            "sharpen" => self.sharpen = check(0.0, 1.0)?,
            other => {
                return Err(CoreError::UnknownParam(format!("layer.{other}")));
            }
        }
        Ok(())
    }

    /// Gets a local adjustment scalar by canonical name.
    pub fn get_by_name(&self, key: &str) -> Result<f32, CoreError> {
        match key.trim().to_ascii_lowercase().as_str() {
            "exposure" => Ok(self.exposure),
            "contrast" => Ok(self.contrast),
            "brightness" => Ok(self.brightness),
            "highlights" => Ok(self.highlights),
            "shadows" => Ok(self.shadows),
            "temperature" | "temp" => Ok(self.temperature),
            "tint" => Ok(self.tint),
            "saturation" | "sat" => Ok(self.saturation),
            "clarity" => Ok(self.clarity),
            "dehaze" => Ok(self.dehaze),
            "sharpen" => Ok(self.sharpen),
            other => Err(CoreError::UnknownParam(format!("layer.{other}"))),
        }
    }

    /// Packs into 3 × `vec4<f32>` matching `shaders/develop.wgsl`:
    /// - `[exposure, contrast, brightness, highlights]`
    /// - `[shadows, temperature, tint, saturation]`
    /// - `[clarity, dehaze, sharpen, active (0/1)]`
    #[inline]
    pub fn to_uniforms(&self) -> [[f32; 4]; 3] {
        let s = self.sanitized();
        let active = if s.is_neutral() { 0.0 } else { 1.0 };
        [
            [s.exposure, s.contrast, s.brightness, s.highlights],
            [s.shadows, s.temperature, s.tint, s.saturation],
            [s.clarity, s.dehaze, s.sharpen, active],
        ]
    }
}

/// Capture One-style local Adjustment Layer with composable masks and per-layer opacity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdjustmentLayer {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_one")]
    pub opacity: f32,
    #[serde(default)]
    pub inverted: bool,
    #[serde(default)]
    pub masks: Vec<LayerMaskItem>,
    #[serde(default)]
    pub adjustments: LocalAdjustments,
}

impl AdjustmentLayer {
    /// Creates an empty enabled adjustment layer (`opacity: 1.0`).
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            enabled: true,
            opacity: 1.0,
            inverted: false,
            masks: Vec::new(),
            adjustments: LocalAdjustments::default(),
        }
    }

    /// Creates a filled adjustment layer with a single `MaskShape::Filled` mask.
    pub fn new_filled(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self::new_with_mask(id, name, MaskShape::Filled)
    }

    /// Creates an adjustment layer initialized with one mask shape.
    pub fn new_with_mask(id: impl Into<String>, name: impl Into<String>, shape: MaskShape) -> Self {
        let mut layer = Self::new(id, name);
        layer.masks.push(LayerMaskItem::new(shape));
        layer
    }

    pub fn sanitized(&self) -> Self {
        let opacity = if self.opacity.is_finite() {
            self.opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let masks: Vec<LayerMaskItem> = self
            .masks
            .iter()
            .take(MAX_MASKS_PER_LAYER)
            .map(LayerMaskItem::sanitized)
            .collect();
        Self {
            id: if self.id.trim().is_empty() {
                "layer".to_owned()
            } else {
                self.id.trim().to_owned()
            },
            name: if self.name.trim().is_empty() {
                "Layer".to_owned()
            } else {
                self.name.trim().to_owned()
            },
            enabled: self.enabled,
            opacity,
            inverted: self.inverted,
            masks,
            adjustments: self.adjustments.sanitized(),
        }
    }

    /// Short display badge describing the layer's active mask configuration.
    pub fn primary_mask_badge(&self) -> &'static str {
        let enabled: Vec<&LayerMaskItem> = self.masks.iter().filter(|m| m.enabled).collect();
        match enabled.len() {
            0 => "Empty",
            1 => enabled[0].shape.kind_label(),
            _ => "Combined",
        }
    }

    /// Returns `true` when this layer can visibly modify image pixels.
    pub fn is_active(&self) -> bool {
        self.enabled && self.opacity > 1e-5 && !self.adjustments.is_neutral()
    }
}

/// Retouching mode for a [`HealSpot`] blemish/object stamp.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealMode {
    /// Seamless brightness-matched healing (matches perimeter luminance ratio).
    #[default]
    Heal,
    /// Direct feathered pixel cloning from `source` onto `target`.
    Clone,
}

impl HealMode {
    pub const ALL: [Self; 2] = [Self::Heal, Self::Clone];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Heal => "Heal",
            Self::Clone => "Clone",
        }
    }

    pub const fn cli_slug(self) -> &'static str {
        match self {
            Self::Heal => "heal",
            Self::Clone => "clone",
        }
    }

    pub fn from_cli_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "heal" | "spot_heal" => Some(Self::Heal),
            "clone" | "stamp" => Some(Self::Clone),
            _ => None,
        }
    }

    #[inline]
    pub const fn to_uniform_f32(self) -> f32 {
        match self {
            Self::Heal => 0.0,
            Self::Clone => 1.0,
        }
    }
}

/// Circular Spot Heal / Clone stamp anchored in normalized `[0, 1]^2` source image UV space.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HealSpot {
    pub id: String,
    pub target: [f32; 2],
    pub source: [f32; 2],
    #[serde(default = "default_heal_radius")]
    pub radius: f32,
    #[serde(default = "default_heal_feather")]
    pub feather: f32,
    #[serde(default = "default_one")]
    pub opacity: f32,
    #[serde(default)]
    pub mode: HealMode,
}

impl HealSpot {
    /// Suggests a clean nearby donor `source` UV for a given `target` UV and `radius`.
    pub fn auto_source_for_target(target: [f32; 2], radius: f32) -> [f32; 2] {
        let tx = if target[0].is_finite() {
            target[0].clamp(0.0, 1.0)
        } else {
            0.5
        };
        let ty = if target[1].is_finite() {
            target[1].clamp(0.0, 1.0)
        } else {
            0.5
        };
        let r = if radius.is_finite() {
            radius.clamp(0.002, 0.25)
        } else {
            default_heal_radius()
        };
        let offset = (r * 2.4).max(0.04);
        let sx = if tx + offset <= 0.96 {
            tx + offset
        } else {
            (tx - offset).max(0.04)
        };
        [sx, ty]
    }

    pub fn sanitized(&self) -> Self {
        let c01 = |v: f32, def: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                def
            }
        };
        let cf = |v: f32, def: f32, lo: f32, hi: f32| {
            if v.is_finite() { v.clamp(lo, hi) } else { def }
        };
        Self {
            id: if self.id.trim().is_empty() {
                "spot".to_owned()
            } else {
                self.id.trim().to_owned()
            },
            target: [c01(self.target[0], 0.5), c01(self.target[1], 0.5)],
            source: [c01(self.source[0], 0.58), c01(self.source[1], 0.5)],
            radius: cf(self.radius, default_heal_radius(), 0.002, 0.25),
            feather: cf(self.feather, default_heal_feather(), 0.0, 1.0),
            opacity: cf(self.opacity, 1.0, 0.0, 1.0),
            mode: self.mode,
        }
    }

    /// Packs into 2 × `vec4<f32>` for GPU uniforms:
    /// - `[target_x, target_y, source_x, source_y]`
    /// - `[radius, feather, opacity, mode (0=Heal, 1=Clone)]`
    #[inline]
    pub fn to_uniforms(&self) -> [[f32; 4]; 2] {
        let s = self.sanitized();
        [
            [s.target[0], s.target[1], s.source[0], s.source[1]],
            [s.radius, s.feather, s.opacity, s.mode.to_uniform_f32()],
        ]
    }
}

/// Extracted per-layer live Luma Range mask parameters shared identically between GPU & CPU.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayerLumaRangeSpec {
    pub enabled: bool,
    pub low: f32,
    pub high: f32,
    pub low_falloff: f32,
    pub high_falloff: f32,
    pub op: MaskCombineOp,
    pub inverted: bool,
}

impl LayerLumaRangeSpec {
    pub fn to_uniforms(&self) -> ([f32; 4], [f32; 4]) {
        (
            [self.low, self.high, self.low_falloff, self.high_falloff],
            [
                if self.enabled { 1.0 } else { 0.0 },
                self.op.to_uniform_f32(),
                if self.inverted { 1.0 } else { 0.0 },
                0.0,
            ],
        )
    }
}

/// Baked `2048×1024` `R8Unorm` mask atlas (4×2 grid of `512×512` tiles for up to 8 layers)
/// plus per-layer spatial-mask presence flags and live Luma Range specifications.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerMaskAtlas {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub layer_has_spatial: [bool; MAX_ADJUSTMENT_LAYERS],
    pub luma_ranges: [LayerLumaRangeSpec; MAX_ADJUSTMENT_LAYERS],
}

impl LayerMaskAtlas {
    /// Creates an empty zeroed `2048×1024` atlas.
    pub fn empty() -> Self {
        Self {
            width: LAYER_MASK_ATLAS_WIDTH,
            height: LAYER_MASK_ATLAS_HEIGHT,
            data: vec![0u8; (LAYER_MASK_ATLAS_WIDTH as usize) * (LAYER_MASK_ATLAS_HEIGHT as usize)],
            layer_has_spatial: [false; MAX_ADJUSTMENT_LAYERS],
            luma_ranges: [LayerLumaRangeSpec::default(); MAX_ADJUSTMENT_LAYERS],
        }
    }

    /// Samples layer `layer_idx`'s `512×512` spatial tile at normalized UV `(u, v)` in `[0, 1]^2`
    /// using bilinear interpolation clamped strictly to texel centers (matching WGSL hardware
    /// `Linear` sampling with zero cross-tile bleed).
    #[inline]
    pub fn sample_tile(&self, layer_idx: usize, u: f32, v: f32) -> f32 {
        if layer_idx >= MAX_ADJUSTMENT_LAYERS {
            return 0.0;
        }
        let tile_col = layer_idx % 4;
        let tile_row = layer_idx / 4;
        let base_x = tile_col * LAYER_MASK_TILE_SIZE;
        let base_y = tile_row * LAYER_MASK_TILE_SIZE;
        let w = self.width as usize;

        let fx = u.clamp(0.0, 1.0) * (LAYER_MASK_TILE_SIZE - 1) as f32;
        let fy = v.clamp(0.0, 1.0) * (LAYER_MASK_TILE_SIZE - 1) as f32;
        let x0 = fx.floor() as usize;
        let y0 = fy.floor() as usize;
        let x1 = (x0 + 1).min(LAYER_MASK_TILE_SIZE - 1);
        let y1 = (y0 + 1).min(LAYER_MASK_TILE_SIZE - 1);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;

        let read = |lx: usize, ly: usize| -> f32 {
            self.data[(base_y + ly) * w + (base_x + lx)] as f32 * (1.0 / 255.0)
        };
        let p00 = read(x0, y0);
        let p10 = read(x1, y0);
        let p01 = read(x0, y1);
        let p11 = read(x1, y1);
        let a = p00 + (p10 - p00) * tx;
        let b = p01 + (p11 - p01) * tx;
        (a + (b - a) * ty).clamp(0.0, 1.0)
    }

    /// Evaluates the complete effective mask weight (including live Luma Range, layer inversion,
    /// and layer opacity) for `layer_idx` at `(u, v)` and perceptual tone `luma_tone` in `[0, 1]`.
    /// Matches `eval_layer_mask` in `shaders/develop.wgsl` bit-for-bit.
    #[inline]
    pub fn evaluate_layer_mask(
        &self,
        layer_idx: usize,
        u: f32,
        v: f32,
        luma_tone: f32,
        layer_inverted: bool,
        layer_opacity: f32,
    ) -> f32 {
        let raw = self.evaluate_layer_mask_raw(layer_idx, u, v, luma_tone, layer_inverted);
        (raw * layer_opacity.clamp(0.0, 1.0)).clamp(0.0, 1.0)
    }

    /// Evaluates the post-invert, pre-opacity mask weight (also used for Red/Grayscale mask overlay).
    #[inline]
    pub fn evaluate_layer_mask_raw(
        &self,
        layer_idx: usize,
        u: f32,
        v: f32,
        luma_tone: f32,
        layer_inverted: bool,
    ) -> f32 {
        if layer_idx >= MAX_ADJUSTMENT_LAYERS {
            return 0.0;
        }
        let has_spatial = self.layer_has_spatial[layer_idx];
        let mut m = if has_spatial {
            self.sample_tile(layer_idx, u, v)
        } else {
            0.0
        };
        let lr = self.luma_ranges[layer_idx];
        if lr.enabled {
            let mut lm =
                luma_range_mask(luma_tone, lr.low, lr.high, lr.low_falloff, lr.high_falloff);
            if lr.inverted {
                lm = 1.0 - lm;
            }
            if has_spatial {
                m = lr.op.combine(m, lm);
            } else {
                m = match lr.op {
                    MaskCombineOp::Add | MaskCombineOp::Intersect => lm,
                    MaskCombineOp::Subtract => (1.0 - lm).clamp(0.0, 1.0),
                };
            }
        }
        if layer_inverted {
            m = 1.0 - m;
        }
        m.clamp(0.0, 1.0)
    }
}

/// Smoothstep helper matching WGSL `smoothstep(edge0, edge1, x)`.
#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if (edge1 - edge0).abs() <= 1e-6 {
        return if x >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Evaluates a smooth Luma Range mask on perceptual tone `luma_tone` (`0..=1`),
/// matching `luma_range_mask` in `shaders/develop.wgsl` identically.
#[inline]
pub fn luma_range_mask(
    luma_tone: f32,
    low: f32,
    high: f32,
    low_falloff: f32,
    high_falloff: f32,
) -> f32 {
    let y = luma_tone.clamp(0.0, 1.0);
    let lo = low.clamp(0.0, 1.0);
    let hi = high.clamp(lo, 1.0);
    let lf = low_falloff.clamp(0.0, 1.0);
    let hf = high_falloff.clamp(0.0, 1.0);

    let lo_w = if lf <= 1e-5 {
        if y >= lo { 1.0 } else { 0.0 }
    } else {
        smoothstep(lo - lf, lo, y)
    };
    let hi_w = if hf <= 1e-5 {
        if y <= hi { 1.0 } else { 0.0 }
    } else {
        1.0 - smoothstep(hi, hi + hf, y)
    };
    (lo_w * hi_w).clamp(0.0, 1.0)
}

/// Evaluates a single spatial [`MaskShape`] at normalized UV `(u, v)` in `[0, 1]^2`.
/// For `MaskShape::LumaRange`, returns `1.0` (since Luma Range evaluates per-pixel on live luma).
pub fn evaluate_spatial_mask_shape(shape: &MaskShape, u: f32, v: f32) -> f32 {
    match shape {
        MaskShape::Filled => 1.0,
        MaskShape::LinearGradient {
            start,
            end,
            feather,
        } => {
            let dx = end[0] - start[0];
            let dy = end[1] - start[1];
            let len2 = dx * dx + dy * dy;
            if len2 <= 1e-7 {
                return 1.0;
            }
            let proj = ((u - start[0]) * dx + (v - start[1]) * dy) / len2;
            let f = feather.clamp(0.0, 1.0);
            let half_w = (f * 0.5).max(0.002);
            let lo = (0.5 - half_w).min(0.5);
            let hi = (0.5 + half_w).max(0.5);
            // Smoothly transitions from 1.0 at start (proj <= lo) to 0.0 at end (proj >= hi).
            1.0 - smoothstep(lo, hi, proj)
        }
        MaskShape::RadialGradient {
            center,
            radius,
            angle_deg,
            feather,
        } => {
            let rad = angle_deg.to_radians();
            let c = rad.cos();
            let s = rad.sin();
            let dx = u - center[0];
            let dy = v - center[1];
            let rx = dx * c + dy * s;
            let ry = -dx * s + dy * c;
            let nx = rx / radius[0].max(0.01);
            let ny = ry / radius[1].max(0.01);
            let d = (nx * nx + ny * ny).sqrt();
            let inner = (1.0 - feather.clamp(0.0, 1.0)).clamp(0.0, 0.995);
            1.0 - smoothstep(inner, 1.0, d)
        }
        MaskShape::BrushStrokes { strokes } => {
            let mut acc = 0.0_f32;
            for dab in strokes.iter().take(MAX_BRUSH_DABS_PER_MASK) {
                let d = dab.sanitized();
                let dist = (u - d.x).hypot(v - d.y);
                if dist < d.radius {
                    let inner = d.radius * d.hardness;
                    let w = (1.0 - smoothstep(inner, d.radius, dist)) * d.opacity;
                    if d.erase {
                        acc *= 1.0 - w;
                    } else {
                        acc = 1.0 - (1.0 - acc) * (1.0 - w);
                    }
                }
            }
            acc.clamp(0.0, 1.0)
        }
        MaskShape::RasterMask { .. } => {
            if let Some((rw, rh, bytes)) = shape.decode_raster_u8() {
                sample_raster_u8(&bytes, rw, rh, u, v)
            } else {
                0.0
            }
        }
        MaskShape::LumaRange { .. } => 1.0,
    }
}

#[inline]
fn sample_raster_u8(bytes: &[u8], width: u32, height: u32, u: f32, v: f32) -> f32 {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 || bytes.len() < w * h {
        return 0.0;
    }
    let fx = u.clamp(0.0, 1.0) * (w.saturating_sub(1)) as f32;
    let fy = v.clamp(0.0, 1.0) * (h.saturating_sub(1)) as f32;
    let x0 = fx.floor() as usize;
    let y0 = fy.floor() as usize;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;
    let get = |x: usize, y: usize| bytes[y * w + x] as f32 * (1.0 / 255.0);
    let a = get(x0, y0) + (get(x1, y0) - get(x0, y0)) * tx;
    let b = get(x0, y1) + (get(x1, y1) - get(x0, y1)) * tx;
    (a + (b - a) * ty).clamp(0.0, 1.0)
}

/// Bakes all spatial masks (`Filled`, `LinearGradient`, `RadialGradient`, `BrushStrokes`,
/// `RasterMask`) across up to 8 [`AdjustmentLayer`]s into the unified `2048×1024` `R8Unorm`
/// [`LayerMaskAtlas`], while extracting per-layer live [`LayerLumaRangeSpec`] uniforms.
pub fn bake_layer_mask_atlas(layers: &[AdjustmentLayer]) -> LayerMaskAtlas {
    type DecodedRaster = (u32, u32, Vec<u8>);
    let mut atlas = LayerMaskAtlas::empty();
    let atlas_w = atlas.width as usize;

    for (layer_idx, layer) in layers.iter().take(MAX_ADJUSTMENT_LAYERS).enumerate() {
        let clean_layer = layer.sanitized();
        let mut spatial_items: Vec<(LayerMaskItem, Option<DecodedRaster>)> = Vec::new();

        for item in &clean_layer.masks {
            if !item.enabled {
                continue;
            }
            if let MaskShape::LumaRange {
                low,
                high,
                low_falloff,
                high_falloff,
            } = item.shape
            {
                if !atlas.luma_ranges[layer_idx].enabled {
                    atlas.luma_ranges[layer_idx] = LayerLumaRangeSpec {
                        enabled: true,
                        low,
                        high,
                        low_falloff,
                        high_falloff,
                        op: item.op,
                        inverted: item.inverted,
                    };
                }
            } else {
                let decoded_raster = item.shape.decode_raster_u8();
                spatial_items.push((item.clone(), decoded_raster));
            }
        }

        if spatial_items.is_empty() {
            continue;
        }
        atlas.layer_has_spatial[layer_idx] = true;

        let tile_col = layer_idx % 4;
        let tile_row = layer_idx / 4;
        let base_x = tile_col * LAYER_MASK_TILE_SIZE;
        let base_y = tile_row * LAYER_MASK_TILE_SIZE;
        let denom = (LAYER_MASK_TILE_SIZE - 1) as f32;

        for ly in 0..LAYER_MASK_TILE_SIZE {
            let v = ly as f32 / denom;
            let row_off = (base_y + ly) * atlas_w + base_x;
            for lx in 0..LAYER_MASK_TILE_SIZE {
                let u = lx as f32 / denom;
                let mut acc = 0.0_f32;
                let mut has_any = false;
                for (item, raster_opt) in &spatial_items {
                    let mut val = if let Some((rw, rh, bytes)) = raster_opt {
                        sample_raster_u8(bytes, *rw, *rh, u, v)
                    } else if matches!(item.shape, MaskShape::RasterMask { .. }) {
                        // Invalid/undecodable raster mask was already checked once above; never
                        // fall back to `evaluate_spatial_mask_shape` (which would re-decode per pixel).
                        0.0
                    } else {
                        evaluate_spatial_mask_shape(&item.shape, u, v)
                    };
                    if item.inverted {
                        val = 1.0 - val;
                    }
                    if !has_any {
                        acc = match item.op {
                            MaskCombineOp::Add | MaskCombineOp::Intersect => val,
                            MaskCombineOp::Subtract => (1.0 - val).clamp(0.0, 1.0),
                        };
                        has_any = true;
                    } else {
                        acc = item.op.combine(acc, val);
                    }
                }
                atlas.data[row_off + lx] = (acc.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }

    atlas
}

/// Evaluates [`LocalAdjustments`] in scene-referred linear light on a single pixel, blending by
/// `mask_weight` (`0.0..=1.0`). Uses precomputed `local_mid_ratio` (for clarity) and `local_hp_rel`
/// (for sharpen) so GPU (`shaders/develop.wgsl`) and CPU (`crates/decode/src/export.rs`) match
/// within `< 2e-3`.
#[inline]
pub fn apply_local_adjustments_linear(
    rgb: [f32; 3],
    adj: &LocalAdjustments,
    mask_weight: f32,
    local_mid_ratio: f32,
    local_hp_rel: f32,
) -> [f32; 3] {
    let w = mask_weight.clamp(0.0, 1.0);
    if w <= 1e-5 || adj.is_neutral() {
        return rgb;
    }

    let mut c = [rgb[0].max(0.0), rgb[1].max(0.0), rgb[2].max(0.0)];

    // 1. Local WB + Exposure
    if adj.exposure.abs() > 1e-5 || adj.temperature.abs() > 1e-5 || adj.tint.abs() > 1e-5 {
        let exp_g = 2.0_f32.powf(adj.exposure);
        let wb_r = 2.0_f32.powf(adj.temperature * 0.45 + adj.tint * 0.22) * exp_g;
        let wb_g = 2.0_f32.powf(-adj.tint * 0.35) * exp_g;
        let wb_b = 2.0_f32.powf(-adj.temperature * 0.45 + adj.tint * 0.22) * exp_g;
        c[0] *= wb_r;
        c[1] *= wb_g;
        c[2] *= wb_b;
    }

    // 2. Local Tonal Regions (Shadows / Highlights / Midtone Brightness)
    if adj.shadows.abs() > 1e-5 || adj.highlights.abs() > 1e-5 || adj.brightness.abs() > 1e-5 {
        let luma = (0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]).max(1e-5);
        let t = (luma / (luma + 0.18)).clamp(0.0, 1.0);
        let shadow_w = (1.0 - t) * (1.0 - t);
        let highlight_w = t * t;
        let mid_w = 4.0 * t * (1.0 - t);
        let stops = adj.shadows * 1.15 * shadow_w
            + adj.highlights * 1.15 * highlight_w
            + adj.brightness * 0.95 * mid_w;
        let gain = 2.0_f32.powf(stops);
        c[0] *= gain;
        c[1] *= gain;
        c[2] *= gain;
    }

    // 3. Local Contrast around 18% middle gray pivot
    if adj.contrast.abs() > 1e-5 {
        let pivot = 0.18_f32;
        let c_pow = 2.0_f32.powf(adj.contrast * 0.85);
        for ch in &mut c {
            *ch = pivot * (*ch / pivot).max(0.0).powf(c_pow);
        }
    }

    // 4. Local Dehaze (matching global dehaze)
    if adj.dehaze.abs() > 1e-5 {
        let dark = c[0].min(c[1]).min(c[2]).max(0.0);
        let veil = (dark / (dark + 0.25)).clamp(0.0, 1.0);
        let t_min = (1.0 - adj.dehaze * 0.52 * veil).clamp(0.32, 1.65);
        let airlight = 0.42_f32;
        for ch in &mut c {
            *ch = ((*ch - airlight * (1.0 - t_min)) / t_min).max(0.0);
        }
        let y = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let dh_sat = (1.0 + adj.dehaze * 0.16).max(0.0);
        for ch in &mut c {
            *ch = (y + (*ch - y) * dh_sat).max(0.0);
        }
    }

    // 5. Local Clarity & Sharpen (using precomputed neighborhood contrast ratios)
    if adj.clarity.abs() > 1e-5 {
        let y = (0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]).max(1e-5);
        let t = (y / (y + 0.18)).clamp(0.0, 1.0);
        let mid_w = (4.0 * t * (1.0 - t)).clamp(0.2, 1.0);
        let gain = local_mid_ratio
            .clamp(0.45, 2.2)
            .powf(adj.clarity * 0.65 * mid_w);
        c[0] *= gain;
        c[1] *= gain;
        c[2] *= gain;
    }
    if adj.sharpen > 1e-4 {
        let sharp_gain = (1.0 + local_hp_rel.clamp(-0.45, 0.45) * adj.sharpen * 1.25).max(0.0);
        c[0] *= sharp_gain;
        c[1] *= sharp_gain;
        c[2] *= sharp_gain;
    }

    // 6. Local Saturation
    if adj.saturation.abs() > 1e-5 {
        let y = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let sat_scale = (1.0 + adj.saturation).max(0.0);
        for ch in &mut c {
            *ch = (y + (*ch - y) * sat_scale).max(0.0);
        }
    }

    [
        rgb[0] + (c[0] - rgb[0]) * w,
        rgb[1] + (c[1] - rgb[1]) * w,
        rgb[2] + (c[2] - rgb[2]) * w,
    ]
}

/// Zero-dependency RFC 4648 standard base64 encoder for compact R8 raster masks.
pub fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | (input[i + 2] as u32);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push(TABLE[(n & 63) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push('=');
    }
    out
}

/// Zero-dependency RFC 4648 standard base64 decoder.
pub fn decode_base64(input: &str) -> Option<Vec<u8>> {
    fn val(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let clean: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !clean.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity((clean.len() / 4) * 3);
    for chunk in clean.as_chunks::<4>().0 {
        let a = val(chunk[0])?;
        let b = val(chunk[1])?;
        out.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            let c = val(chunk[2])?;
            out.push((b << 4) | (c >> 2));
            if chunk[3] != b'=' {
                let d = val(chunk[3])?;
                out.push((c << 6) | d);
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_and_raster_mask_roundtrip() {
        let raw: Vec<u8> = (0..64u8).map(|i| i.wrapping_mul(7)).collect();
        let b64 = encode_base64(&raw);
        assert_eq!(decode_base64(&b64).unwrap(), raw);

        let shape = MaskShape::from_raster_u8(8, 8, &raw);
        let (w, h, dec) = shape.decode_raster_u8().unwrap();
        assert_eq!((w, h), (8, 8));
        assert_eq!(dec, raw);
    }

    #[test]
    fn combine_masks_add_subtract_intersect_and_luma_range() {
        // Linear top-half mask + Radial center subtract -> center is carved out, top corners remain 1.0.
        let mut layer = AdjustmentLayer::new("l1", "Sky");
        layer.masks.push(LayerMaskItem {
            enabled: true,
            op: MaskCombineOp::Add,
            inverted: false,
            shape: MaskShape::LinearGradient {
                start: [0.5, 0.0],
                end: [0.5, 0.8],
                feather: 0.2,
            },
        });
        layer.masks.push(LayerMaskItem {
            enabled: true,
            op: MaskCombineOp::Subtract,
            inverted: false,
            shape: MaskShape::RadialGradient {
                center: [0.5, 0.2],
                radius: [0.15, 0.15],
                angle_deg: 0.0,
                feather: 0.2,
            },
        });

        let atlas = bake_layer_mask_atlas(&[layer]);
        assert!(atlas.layer_has_spatial[0]);
        let top_left = atlas.sample_tile(0, 0.1, 0.1);
        let carved_center = atlas.sample_tile(0, 0.5, 0.2);
        let bottom = atlas.sample_tile(0, 0.5, 0.9);
        assert!(top_left > 0.95, "top_left={top_left}");
        assert!(carved_center < 0.05, "carved_center={carved_center}");
        assert!(bottom < 0.05, "bottom={bottom}");

        // Brush paint + erase dabs.
        let brush = MaskShape::BrushStrokes {
            strokes: vec![
                BrushDab {
                    x: 0.5,
                    y: 0.5,
                    radius: 0.2,
                    hardness: 0.8,
                    opacity: 1.0,
                    erase: false,
                },
                BrushDab {
                    x: 0.5,
                    y: 0.5,
                    radius: 0.08,
                    hardness: 0.9,
                    opacity: 1.0,
                    erase: true,
                },
            ],
        };
        assert!(evaluate_spatial_mask_shape(&brush, 0.62, 0.5) > 0.8);
        assert!(evaluate_spatial_mask_shape(&brush, 0.5, 0.5) < 0.05);

        // Luma Range mask isolates highlights cleanly.
        assert!(luma_range_mask(0.8, 0.6, 1.0, 0.1, 0.0) > 0.99);
        assert!(luma_range_mask(0.2, 0.6, 1.0, 0.1, 0.0) < 0.01);
        let mid_falloff = luma_range_mask(0.55, 0.6, 1.0, 0.1, 0.0);
        assert!(mid_falloff > 0.3 && mid_falloff < 0.7);
    }

    #[test]
    fn local_adjustments_and_heal_spot_sanitization() {
        let mut adj = LocalAdjustments::default();
        assert!(adj.is_neutral());
        adj.set_by_name("exposure", 1.0).unwrap();
        assert!(adj.set_by_name("exposure", 9.0).is_err());
        assert!(adj.set_by_name("nonexistent", 0.5).is_err());
        assert!((adj.get_by_name("exposure").unwrap() - 1.0).abs() < 1e-6);

        let base = [0.18, 0.18, 0.18];
        let full = apply_local_adjustments_linear(base, &adj, 1.0, 1.0, 0.0);
        let half = apply_local_adjustments_linear(base, &adj, 0.5, 1.0, 0.0);
        assert!((full[0] - 0.36).abs() < 1e-4);
        assert!((half[0] - 0.27).abs() < 1e-4);

        let spot = HealSpot {
            id: "h1".to_owned(),
            target: [0.5, 0.5],
            source: HealSpot::auto_source_for_target([0.5, 0.5], 0.04),
            radius: 0.04,
            feather: 0.5,
            opacity: 1.0,
            mode: HealMode::Heal,
        };
        assert!(spot.source[0] > 0.55);
        assert_eq!(spot.to_uniforms()[1][3], 0.0);

        // DoS hardening: BrushStrokes truncates to MAX_BRUSH_DABS_PER_MASK and oversized RasterMask b64 is rejected.
        let huge_brush = MaskShape::BrushStrokes {
            strokes: vec![BrushDab::default(); MAX_BRUSH_DABS_PER_MASK + 250],
        };
        if let MaskShape::BrushStrokes { strokes } = huge_brush.sanitized() {
            assert_eq!(strokes.len(), MAX_BRUSH_DABS_PER_MASK);
        } else {
            panic!("expected BrushStrokes");
        }
        let oversized_raster = MaskShape::RasterMask {
            width: 256,
            height: 256,
            data_b64: "A".repeat(MAX_RASTER_MASK_B64_LEN + 4),
        };
        assert!(oversized_raster.decode_raster_u8().is_none());
    }

    #[test]
    fn atlas_512_tile_bake_and_legacy_256_raster_resample() {
        assert_eq!(LAYER_MASK_TILE_SIZE, 512);
        assert_eq!(LAYER_MASK_ATLAS_WIDTH, 2048);
        assert_eq!(LAYER_MASK_ATLAS_HEIGHT, 1024);

        // 1) Native 512x512 RasterMask bakes 1:1 into tile 0.
        let mut ramp_512 = vec![0u8; 512 * 512];
        for y in 0..512usize {
            for x in 0..512usize {
                ramp_512[y * 512 + x] = ((x * 255 + 255) / 511) as u8;
            }
        }
        let layer_512 = AdjustmentLayer::new_with_mask(
            "l512",
            "AI 512",
            MaskShape::from_raster_u8(512, 512, &ramp_512),
        );

        // 2) Legacy 256x256 RasterMask deserialized from sidecar JSON bilinearly resamples into 512x512 tile 1.
        let mut ramp_256 = vec![0u8; 256 * 256];
        for y in 0..256usize {
            for x in 0..256usize {
                ramp_256[y * 256 + x] = y as u8;
            }
        }
        let legacy_json = serde_json::json!({
            "type": "raster_mask",
            "width": 256,
            "height": 256,
            "data_b64": encode_base64(&ramp_256),
        });
        let legacy_shape: MaskShape = serde_json::from_value(legacy_json).unwrap();
        let layer_256 = AdjustmentLayer::new_with_mask("l256", "Legacy 256", legacy_shape);

        let atlas = bake_layer_mask_atlas(&[layer_512, layer_256]);
        assert_eq!(atlas.width, 2048);
        assert_eq!(atlas.height, 1024);
        assert_eq!(atlas.data.len(), 2048 * 1024);
        assert!(atlas.layer_has_spatial[0]);
        assert!(atlas.layer_has_spatial[1]);

        // Verify 512x512 horizontal ramp endpoints and midpoint
        assert!(atlas.sample_tile(0, 0.0, 0.5) < 0.01);
        assert!((atlas.sample_tile(0, 0.5, 0.5) - 0.5).abs() < 0.01);
        assert!(atlas.sample_tile(0, 1.0, 0.5) > 0.99);

        // Verify legacy 256x256 vertical ramp bilinearly resampled cleanly into 512x512 tile 1
        assert!(atlas.sample_tile(1, 0.5, 0.0) < 0.01);
        assert!((atlas.sample_tile(1, 0.5, 0.25) - 0.25).abs() < 0.01);
        assert!((atlas.sample_tile(1, 0.5, 0.5) - 0.5).abs() < 0.01);
        assert!((atlas.sample_tile(1, 0.5, 0.75) - 0.75).abs() < 0.01);
        assert!(atlas.sample_tile(1, 0.5, 1.0) > 0.99);

        // 3) DoS dimension cap: 1024x1024 allowed, >1024 rejected by decode_raster_u8 & clamped by sanitized().
        let huge_dims = MaskShape::RasterMask {
            width: MAX_RASTER_MASK_DIM + 1,
            height: MAX_RASTER_MASK_DIM + 1,
            data_b64: encode_base64(&[255u8; 16]),
        };
        assert!(huge_dims.decode_raster_u8().is_none());
        if let MaskShape::RasterMask { width, height, .. } = huge_dims.sanitized() {
            assert_eq!(width, MAX_RASTER_MASK_DIM);
            assert_eq!(height, MAX_RASTER_MASK_DIM);
        } else {
            panic!("expected RasterMask");
        }

        // 4) DoS hardening: invalid-base64 2 MiB RasterMask bakes cleanly in O(tile) to 0.0
        // without re-decoding per pixel.
        let mut bad_b64 = "A".repeat(MAX_RASTER_MASK_B64_LEN - 4);
        bad_b64.push_str("!!!!");
        let bad_layer = AdjustmentLayer::new_with_mask(
            "l_bad_b64",
            "Bad B64",
            MaskShape::RasterMask {
                width: 512,
                height: 512,
                data_b64: bad_b64,
            },
        );
        let bad_atlas = bake_layer_mask_atlas(&[bad_layer]);
        assert!(bad_atlas.layer_has_spatial[0]);
        assert_eq!(bad_atlas.sample_tile(0, 0.0, 0.0), 0.0);
        assert_eq!(bad_atlas.sample_tile(0, 0.5, 0.5), 0.0);
        assert_eq!(bad_atlas.sample_tile(0, 1.0, 1.0), 0.0);
    }
}
