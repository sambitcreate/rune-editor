//! Application state, session catalog orchestration, undo/redo, Speed Edit,
//! live cursor color readout, interactive crop, and macOS document-state sync.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rune::{AppHandle, LiveRegion, Mods, Pixels, TextEdit, Tx};
use rune_editor_core::{
    ADVANCED_COLOR_SLICE_COUNT, ALL_CLIPBOARD_GROUPS, AdjustmentLayer, AiModelConfig, BrushDab,
    CameraProfileDescriptor, ChannelLevels, ClarityMethod, ColorTag, CompiledRecipe, CropAspect,
    CullFilter, CullFlag, DEFAULT_CAMERA_PROFILE, DEFAULT_PROFILE_STRENGTH, DevelopRecipe,
    FilmCurve, FilmNegativeParams, GrainType, HealMode, HealSpot, HistogramData, HistoryStack,
    LayerMaskItem, LevelsParams, LocalAdjustments, MAX_ADJUSTMENT_LAYERS, MAX_BRUSH_DABS_PER_MASK,
    MAX_HEAL_SPOTS, MAX_MASKS_PER_LAYER, MAX_PROMPT_POINTS, MaskCombineOp, MaskShape, ModelId,
    ModelStatus, PhotoMeta, ProcessingMode, ProfileCategory, PromptBox, PromptPoint,
    SegmentationEngine, SessionCatalog, SessionEntry, SmartMaskPrompt, StylePreset, ToneCurves,
    UserStyle, ViewportCamera, VignetteMethod, WbPreset, all_param_descriptors,
    apply_style_to_recipe, auto_exposure_and_tone, auto_levels, auto_white_balance,
    builtin_presets, canonical_profile_id, compute_histogram_compiled, copy_recipe_groups,
    create_capture_one_session, default_curve_points, delete_user_style, filmic_tonemap,
    find_builtin_profile, find_scalar_param, import_user_profile, is_valid_camera_profile_id,
    linear_to_srgb, list_all_camera_profiles, list_user_styles, load_ai_model_config,
    models_root_dir, modified_recipe_groups, move_photo_with_sidecar, pick_white_balance,
    recipe_to_sparse_patch, rgb_to_hsl_and_lab, save_user_style, stack_style_onto_recipe,
    temperature_to_kelvin, update_ai_model_config, update_ai_model_config_at,
};
use rune_editor_decode::{
    ActiveEngineSummary, ExportColorSpace, ExportFormat, ExportOptions, ExportRecipeSpec,
    HotFolderWatcher, LinearImage, ModelDownloadProgress, NamingContext, RawDecodeMode,
    SmartMaskResult, TextWatermark, ThumbnailRgba8, WatermarkPosition, active_engine_summary,
    builtin_export_recipes, download_model, expand_naming_template, export_image_with_options,
    export_multi_recipe, generate_demo_hdr_scene, generate_demo_thumbnail, generate_heuristic_mask,
    generate_smart_mask, guard_not_source, is_raw_extension, list_model_statuses, load_file,
    load_file_proxy_with_mode, load_from_memory, load_thumbnail, remove_model, scan_photo_folder,
    setup_model_from_local_dir,
};
use rune_editor_mcp::{ActiveCanvasSnapshot, LocalMcpServer, McpService, McpSettings};

use crate::mcp_bridge::GuiMcpBridge;

const SAMPLE_PORTRAIT_BYTES: &[u8] =
    include_bytes!("../../../assets/samples/editorial_portrait.jpg");

use crate::menu::build_menu_items;

/// Effect slot mark ID for the central GPU photo viewport.
pub const PHOTO_VIEWPORT_MARK: u32 = 1;

/// Maximum full-resolution linear images retained in the background neighbor prefetch cache.
pub const MAX_PREFETCH_CACHE_ENTRIES: usize = 2;

/// Canonical `(min, max)` bounds for any top-level scalar parameter in `SCALAR_PARAMS`.
pub fn scalar_bounds(id: &str) -> (f32, f32) {
    let spec = find_scalar_param(id).unwrap_or_else(|| {
        panic!("scalar parameter `{id}` must be registered in rune_editor_core::SCALAR_PARAMS")
    });
    (spec.min, spec.max)
}

/// Inclusive range `min..=max` sourced directly from `rune_editor_core::SCALAR_PARAMS`.
pub fn scalar_range(id: &str) -> std::ops::RangeInclusive<f32> {
    let (min, max) = scalar_bounds(id);
    min..=max
}

/// Canonical default value sourced directly from `rune_editor_core::SCALAR_PARAMS`.
pub fn scalar_default(id: &str) -> f32 {
    find_scalar_param(id)
        .unwrap_or_else(|| {
            panic!("scalar parameter `{id}` must be registered in rune_editor_core::SCALAR_PARAMS")
        })
        .default
}

/// Cached `(min, max, default)` lookup map built once from `all_param_descriptors()` so
/// structured sliders (Advanced Color, Skin Tone, Lens, Keystone) also source bounds from core.
fn param_descriptor_table() -> &'static HashMap<String, (f32, f32, f32)> {
    static TABLE: OnceLock<HashMap<String, (f32, f32, f32)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut map = HashMap::new();
        for d in all_param_descriptors() {
            if let (Some(min), Some(max)) = (d.min, d.max) {
                let def = d
                    .default
                    .as_ref()
                    .and_then(|v| v.as_f64())
                    .map_or(0.0, |v| v as f32);
                map.insert(d.id, (min, max, def));
            }
        }
        map
    })
}

/// Canonical `(min, max)` bounds for any structured dot-path parameter in `all_param_descriptors()`.
pub fn descriptor_bounds(id: &str) -> (f32, f32) {
    let &(min, max, _) = param_descriptor_table().get(id).unwrap_or_else(|| {
        panic!("parameter `{id}` must be registered with min/max in core registry")
    });
    (min, max)
}

/// Inclusive range `min..=max` sourced directly from `rune_editor_core::all_param_descriptors()`.
pub fn descriptor_range(id: &str) -> std::ops::RangeInclusive<f32> {
    let (min, max) = descriptor_bounds(id);
    min..=max
}

/// Canonical default value sourced directly from `rune_editor_core::all_param_descriptors()`.
pub fn descriptor_default(id: &str) -> f32 {
    param_descriptor_table()
        .get(id)
        .unwrap_or_else(|| {
            panic!("parameter `{id}` must be registered with default in core registry")
        })
        .2
}

/// Canonical exposure slider bounds sourced directly from `rune_editor_core`'s parameter registry.
#[cfg(test)]
pub fn exposure_bounds() -> (f32, f32) {
    scalar_bounds("exposure")
}

/// Right inspector tab selection (Capture One / Lightroom darkroom layout).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InspectorTab {
    #[default]
    Light,
    Color,
    Detail,
    Crop,
    Layers,
    Presets,
}

impl InspectorTab {
    pub const ALL: [Self; 6] = [
        Self::Light,
        Self::Color,
        Self::Detail,
        Self::Crop,
        Self::Layers,
        Self::Presets,
    ];
    pub const LABELS: [&'static str; 6] = ["Light", "Color", "Detail", "Lens", "Layers", "Style"];

    pub fn index(self) -> usize {
        match self {
            Self::Light => 0,
            Self::Color => 1,
            Self::Detail => 2,
            Self::Crop => 3,
            Self::Layers => 4,
            Self::Presets => 5,
        }
    }

    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or(Self::Light)
    }
}

/// Active channel inside the interactive Tone Curve Editor (Capture One 5-tab layout).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CurveChannel {
    Rgb,
    #[default]
    Luma,
    Red,
    Green,
    Blue,
}

impl CurveChannel {
    pub const ALL: [Self; 5] = [Self::Rgb, Self::Luma, Self::Red, Self::Green, Self::Blue];

    pub fn label(self) -> &'static str {
        match self {
            Self::Rgb => "RGB",
            Self::Luma => "Luma",
            Self::Red => "Red",
            Self::Green => "Green",
            Self::Blue => "Blue",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Rgb => "RGB",
            Self::Luma => "Luma",
            Self::Red => "R",
            Self::Green => "G",
            Self::Blue => "B",
        }
    }

    pub fn points(self, curves: &ToneCurves) -> &[[f32; 2]] {
        match self {
            Self::Rgb => &curves.rgb,
            Self::Luma => &curves.luma,
            Self::Red => &curves.red,
            Self::Green => &curves.green,
            Self::Blue => &curves.blue,
        }
    }

    pub fn points_mut(self, curves: &mut ToneCurves) -> &mut Vec<[f32; 2]> {
        match self {
            Self::Rgb => &mut curves.rgb,
            Self::Luma => &mut curves.luma,
            Self::Red => &mut curves.red,
            Self::Green => &mut curves.green,
            Self::Blue => &mut curves.blue,
        }
    }

    /// Returns the hovered pixel's normalized `[0.0, 1.0]` tonal value for this curve channel
    /// to position Capture One's live vertical tonal indicator line.
    pub fn probe_tone(self, readout: &CursorColorReadout) -> f32 {
        let r = readout.rgb8[0] as f32 / 255.0;
        let g = readout.rgb8[1] as f32 / 255.0;
        let b = readout.rgb8[2] as f32 / 255.0;
        let v = match self {
            Self::Rgb | Self::Luma => 0.2126 * r + 0.7152 * g + 0.0722 * b,
            Self::Red => r,
            Self::Green => g,
            Self::Blue => b,
        };
        v.clamp(0.0, 1.0)
    }
}

/// Active channel inside the Capture One-style Levels tool (`RGB | R | G | B`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LevelsChannel {
    #[default]
    Rgb,
    Red,
    Green,
    Blue,
}

impl LevelsChannel {
    pub const ALL: [Self; 4] = [Self::Rgb, Self::Red, Self::Green, Self::Blue];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Rgb => "RGB",
            Self::Red => "Red",
            Self::Green => "Green",
            Self::Blue => "Blue",
        }
    }

    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Rgb => "RGB",
            Self::Red => "R",
            Self::Green => "G",
            Self::Blue => "B",
        }
    }

    pub fn channel(self, lv: &LevelsParams) -> ChannelLevels {
        match self {
            Self::Rgb => lv.master_channel().sanitized(),
            Self::Red => lv.red.sanitized(),
            Self::Green => lv.green.sanitized(),
            Self::Blue => lv.blue.sanitized(),
        }
    }

    pub fn is_modified(self, lv: &LevelsParams) -> bool {
        !self.channel(lv).is_identity()
    }

    pub fn set_in_black(self, lv: &mut LevelsParams, v: f32) {
        match self {
            Self::Rgb => lv.in_black = v.min(lv.in_white - 0.02).clamp(0.0, 0.95),
            Self::Red => lv.red.in_black = v.min(lv.red.in_white - 0.02).clamp(0.0, 0.95),
            Self::Green => lv.green.in_black = v.min(lv.green.in_white - 0.02).clamp(0.0, 0.95),
            Self::Blue => lv.blue.in_black = v.min(lv.blue.in_white - 0.02).clamp(0.0, 0.95),
        }
    }

    pub fn set_in_mid(self, lv: &mut LevelsParams, v: f32) {
        let clamped = v.clamp(-1.0, 1.0);
        match self {
            Self::Rgb => lv.in_mid = clamped,
            Self::Red => lv.red.in_mid = clamped,
            Self::Green => lv.green.in_mid = clamped,
            Self::Blue => lv.blue.in_mid = clamped,
        }
    }

    pub fn set_in_white(self, lv: &mut LevelsParams, v: f32) {
        match self {
            Self::Rgb => lv.in_white = v.max(lv.in_black + 0.02).clamp(0.05, 1.0),
            Self::Red => lv.red.in_white = v.max(lv.red.in_black + 0.02).clamp(0.05, 1.0),
            Self::Green => lv.green.in_white = v.max(lv.green.in_black + 0.02).clamp(0.05, 1.0),
            Self::Blue => lv.blue.in_white = v.max(lv.blue.in_black + 0.02).clamp(0.05, 1.0),
        }
    }

    pub fn set_out_black(self, lv: &mut LevelsParams, v: f32) {
        let clamped = v.clamp(0.0, 1.0);
        match self {
            Self::Rgb => lv.out_black = clamped,
            Self::Red => lv.red.out_black = clamped,
            Self::Green => lv.green.out_black = clamped,
            Self::Blue => lv.blue.out_black = clamped,
        }
    }

    pub fn set_out_white(self, lv: &mut LevelsParams, v: f32) {
        let clamped = v.clamp(0.0, 1.0);
        match self {
            Self::Rgb => lv.out_white = clamped,
            Self::Red => lv.red.out_white = clamped,
            Self::Green => lv.green.out_white = clamped,
            Self::Blue => lv.blue.out_white = clamped,
        }
    }
}

/// Active view mode inside the Capture One-style Color Balance card (`3-Way | Master`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorBalanceView {
    #[default]
    ThreeWay,
    Master,
}

/// Active mode inside the Capture One-style 3-tab Color Editor (`Basic | Advanced | Skin Tone`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorEditorMode {
    #[default]
    Basic,
    Advanced,
    SkinTone,
}

impl ColorEditorMode {
    pub const ALL: [Self; 3] = [Self::Basic, Self::Advanced, Self::SkinTone];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Basic => "Basic",
            Self::Advanced => "Advanced",
            Self::SkinTone => "Skin Tone",
        }
    }
}

/// Capture One Speed Edit parameter bound to a conflict-free hold key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeedEditParam {
    Exposure,
    Contrast,
    Brightness,
    Saturation,
    Highlights,
    Shadows,
    Whites,
    Clarity,
    Temperature,
    Tint,
}

impl SpeedEditParam {
    /// Maps lowercase letter hotkeys without conflicting with `0`–`5` ratings,
    /// `P`/`X` culling, `F`/`I` panels, `C` crop, or `\` compare.
    pub fn from_key(ch: &str) -> Option<Self> {
        match ch.to_ascii_lowercase().as_str() {
            "q" => Some(Self::Exposure),
            "w" => Some(Self::Contrast),
            "e" => Some(Self::Brightness),
            "r" => Some(Self::Saturation),
            "a" => Some(Self::Highlights),
            "s" => Some(Self::Shadows),
            "d" => Some(Self::Whites),
            "z" => Some(Self::Clarity),
            "t" => Some(Self::Temperature),
            "y" => Some(Self::Tint),
            _ => None,
        }
    }

    pub const fn key_label(self) -> &'static str {
        match self {
            Self::Exposure => "Q",
            Self::Contrast => "W",
            Self::Brightness => "E",
            Self::Saturation => "R",
            Self::Highlights => "A",
            Self::Shadows => "S",
            Self::Whites => "D",
            Self::Clarity => "Z",
            Self::Temperature => "T",
            Self::Tint => "Y",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Exposure => "Exposure",
            Self::Contrast => "Contrast",
            Self::Brightness => "Brightness",
            Self::Saturation => "Saturation",
            Self::Highlights => "Highlights",
            Self::Shadows => "Shadows",
            Self::Whites => "Whites",
            Self::Clarity => "Clarity",
            Self::Temperature => "Temperature",
            Self::Tint => "Tint",
        }
    }

    pub fn format_value(self, r: &DevelopRecipe) -> String {
        match self {
            Self::Exposure => format!("{:+.2} EV", r.exposure),
            Self::Contrast => format!("{:+.0}", r.contrast * 100.0),
            Self::Brightness => format!("{:+.0}", r.brightness * 100.0),
            Self::Saturation => format!("{:+.0}", r.saturation * 100.0),
            Self::Highlights => format!("{:+.0}", r.highlights * 100.0),
            Self::Shadows => format!("{:+.0}", r.shadows * 100.0),
            Self::Whites => format!("{:+.0}", r.whites * 100.0),
            Self::Clarity => format!("{:+.0}", r.clarity * 100.0),
            Self::Temperature => format!("{} K", temperature_to_kelvin(r.temperature)),
            Self::Tint => format!("{:+.0}", r.tint * 100.0),
        }
    }

    /// Applies a normalized scrub delta (`~1.0` ≈ full comfortable sweep) and returns
    /// `true` if the recipe parameter changed.
    pub fn apply_delta(self, r: &mut DevelopRecipe, delta: f32) -> bool {
        if !delta.is_finite() || delta.abs() < 1e-6 {
            return false;
        }
        let (param_id, scale) = match self {
            Self::Exposure => ("exposure", 3.0),
            Self::Contrast => ("contrast", 1.0),
            Self::Brightness => ("brightness", 1.0),
            Self::Saturation => ("saturation", 1.0),
            Self::Highlights => ("highlights", 1.0),
            Self::Shadows => ("shadows", 1.0),
            Self::Whites => ("whites", 1.0),
            Self::Clarity => ("clarity", 1.0),
            Self::Temperature => ("temperature", 1.0),
            Self::Tint => ("tint", 1.0),
        };
        let (min, max) = scalar_bounds(param_id);
        let slot = match self {
            Self::Exposure => &mut r.exposure,
            Self::Contrast => &mut r.contrast,
            Self::Brightness => &mut r.brightness,
            Self::Saturation => &mut r.saturation,
            Self::Highlights => &mut r.highlights,
            Self::Shadows => &mut r.shadows,
            Self::Whites => &mut r.whites,
            Self::Clarity => &mut r.clarity,
            Self::Temperature => &mut r.temperature,
            Self::Tint => &mut r.tint,
        };
        let prev = *slot;
        *slot = (prev + delta * scale).clamp(min, max);
        let changed = (*slot - prev).abs() > 1e-6;
        if changed && matches!(self, Self::Temperature | Self::Tint) {
            r.wb_preset = WbPreset::Custom;
        }
        changed
    }
}

/// Live Capture One pixel probe values under the cursor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorColorReadout {
    pub rgb8: [u8; 3],
    pub hsl: [f32; 3],
    pub lab: [f32; 3],
}

/// Maximum pinned live Color Readout markers on the photo viewport (`#1..#4`).
pub const MAX_PINNED_READOUTS: usize = 4;

/// A numbered Color Readout marker pinned to a source UV on the canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PinnedReadout {
    pub source_uv: [f32; 2],
    pub slot_uv: [f32; 2],
}

/// Pinned orange-bordered Capture One Compare Variant reference frame (`⌘⏎`).
#[derive(Clone)]
pub struct CompareReference {
    pub label: String,
    pub variant_tag: String,
    pub pixels: Pixels,
}

/// Active handle when dragging the crop frame on the photo canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CropDragHandle {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Left,
    Right,
    Top,
    Bottom,
    Move,
}

/// Active interactive canvas tool for Phase 4 Layers, Masks & Spot Retouching.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CanvasTool {
    #[default]
    Hand,
    AiSelect,
    Brush,
    Eraser,
    LinearGradient,
    RadialGradient,
    Heal,
    Clone,
}

impl CanvasTool {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hand => "Hand",
            Self::AiSelect => "AI Select",
            Self::Brush => "Brush",
            Self::Eraser => "Eraser",
            Self::LinearGradient => "Linear Mask",
            Self::RadialGradient => "Radial Mask",
            Self::Heal => "Heal",
            Self::Clone => "Clone",
        }
    }

    pub const fn hud_badge(self) -> &'static str {
        match self {
            Self::Hand => "HAND",
            Self::AiSelect => "AI SELECT (W)",
            Self::Brush => "BRUSH (B)",
            Self::Eraser => "ERASER (E)",
            Self::LinearGradient => "LINEAR MASK (L)",
            Self::RadialGradient => "RADIAL MASK (T)",
            Self::Heal => "HEAL (Q)",
            Self::Clone => "CLONE (S)",
        }
    }

    pub const fn is_retouch_or_mask(self) -> bool {
        !matches!(self, Self::Hand)
    }
}

/// Transient drag tracking for interactive canvas AI Select/Brush/Eraser/Gradient/Heal operations.
#[derive(Clone, Copy, Debug)]
pub enum LayerDragState {
    AiSelect {
        start_uv: [f32; 2],
        current_uv: [f32; 2],
        start_local: [f32; 2],
        shift_held: bool,
        alt_held: bool,
        dragging_box: bool,
    },
    Brush {
        layer_idx: usize,
        mask_idx: usize,
        erase: bool,
    },
    Linear {
        layer_idx: usize,
        mask_idx: usize,
        start_uv: [f32; 2],
    },
    Radial {
        layer_idx: usize,
        mask_idx: usize,
        center_uv: [f32; 2],
    },
    Heal {
        spot_idx: usize,
    },
}

/// Telemetry from the most recent AI Subject, AI Background, or Interactive AI Select run.
#[derive(Clone, Debug, PartialEq)]
pub struct AiMaskStatus {
    pub engine: SegmentationEngine,
    pub backend_label: String,
    pub elapsed_ms: u64,
    pub iou_score: Option<f32>,
    pub instance_count: Option<usize>,
    pub fallback_note: Option<String>,
}

impl AiMaskStatus {
    pub fn from_result(res: &SmartMaskResult) -> Self {
        Self {
            engine: res.engine_used,
            backend_label: res.backend_label.clone(),
            elapsed_ms: res.elapsed_ms,
            iou_score: res.iou_score,
            instance_count: res.instance_count,
            fallback_note: res.fallback_note.clone(),
        }
    }

    /// Compact single-line badge text, e.g. `Apple Vision ANE · 48ms` or `SAM 2.1 · 18ms · IoU 0.94`.
    pub fn compact_badge(&self) -> String {
        let short_engine = match self.engine {
            SegmentationEngine::AppleVision => "Apple Vision ANE",
            SegmentationEngine::Sam21Tiny => "SAM 2.1",
            SegmentationEngine::Sam3Tracker => "SAM 3",
            SegmentationEngine::Heuristic => "Heuristic",
            SegmentationEngine::Auto => self.backend_label.as_str(),
        };
        let mut parts = vec![short_engine.to_owned(), format!("{}ms", self.elapsed_ms)];
        if let Some(iou) = self.iou_score {
            parts.push(format!("IoU {:.2}", iou));
        }
        parts.join(" · ")
    }
}

/// Lifecycle phase of a background ONNX model download job displayed in Settings → AI Models.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelDownloadPhase {
    Downloading {
        fraction: f32,
        file_index: usize,
        total_files: usize,
        file_name: String,
        bytes_downloaded: u64,
        total_bytes: u64,
        status: String,
    },
    Completed,
    Cancelled,
    Error(String),
}

/// Shared state for an active or recently finished ONNX model download job.
#[derive(Clone, Debug)]
pub struct ModelDownloadJob {
    pub phase: ModelDownloadPhase,
    pub cancel: Arc<AtomicBool>,
}

impl ModelDownloadJob {
    pub fn with_phase(phase: ModelDownloadPhase) -> Self {
        Self {
            phase,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self.phase, ModelDownloadPhase::Downloading { .. })
    }

    pub fn apply_progress(&mut self, progress: ModelDownloadProgress) {
        self.phase = ModelDownloadPhase::Downloading {
            fraction: progress.overall_fraction.clamp(0.0, 1.0),
            file_index: progress.file_index,
            total_files: progress.total_files,
            file_name: progress.file_name,
            bytes_downloaded: progress.bytes_downloaded,
            total_bytes: progress.total_bytes,
            status: progress.status,
        };
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancel_requested(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// RAII registry of active/recent ONNX model download jobs.
///
/// Automatically signals cancellation on every active download's `AtomicBool` when dropped
/// (e.g. on `AppState` drop / window close), without preventing `..AppState::default()` struct
/// update syntax.
#[derive(Default)]
pub struct ModelDownloadRegistry {
    pub jobs: Arc<Mutex<HashMap<ModelId, Arc<Mutex<ModelDownloadJob>>>>>,
}

impl ModelDownloadRegistry {
    pub fn cancel_all(&self) {
        if let Ok(map) = self.jobs.lock() {
            for job_arc in map.values() {
                if let Ok(job) = job_arc.lock() {
                    job.request_cancel();
                }
            }
        }
    }
}

impl Drop for ModelDownloadRegistry {
    fn drop(&mut self) {
        self.cancel_all();
    }
}

/// Active navigation section inside the dedicated Settings view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsSection {
    #[default]
    AgentMcp,
    AiModels,
    CliAndSkill,
    Shortcuts,
    Security,
}

/// Capture One-style configurable neutral viewport Proof Margin (`Compact | Studio | Exhibition`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProofMargin {
    #[default]
    Compact,
    Studio,
    Exhibition,
}

impl ProofMargin {
    pub const ALL: [Self; 3] = [Self::Compact, Self::Studio, Self::Exhibition];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Studio => "Studio",
            Self::Exhibition => "Exhibition",
        }
    }

    pub const fn slug(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Studio => "studio",
            Self::Exhibition => "exhibition",
        }
    }

    pub fn from_slug(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "compact" => Some(Self::Compact),
            "studio" => Some(Self::Studio),
            "exhibition" => Some(Self::Exhibition),
            _ => None,
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Compact => Self::Studio,
            Self::Studio => Self::Exhibition,
            Self::Exhibition => Self::Compact,
        }
    }

    /// Extra outer mat padding in points (adds to shader's ~14pt internal pad).
    pub const fn extra_padding_pts(self) -> f32 {
        match self {
            Self::Compact => 0.0,
            Self::Studio => crate::tokens::size::PROOF_MARGIN_STUDIO,
            Self::Exhibition => crate::tokens::size::PROOF_MARGIN_EXHIBITION,
        }
    }

    pub const fn effective_pt_label(self) -> &'static str {
        match self {
            Self::Compact => "16 pt",
            Self::Studio => "48 pt",
            Self::Exhibition => "84 pt",
        }
    }
}

/// Grouping category inside Settings → Keyboard Shortcuts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutCategory {
    ToolsAndMasks,
    ViewAndOverlays,
    CullingAndNav,
    Workflow,
}

impl ShortcutCategory {
    pub const ALL: [Self; 4] = [
        Self::ToolsAndMasks,
        Self::ViewAndOverlays,
        Self::CullingAndNav,
        Self::Workflow,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::ToolsAndMasks => "TOOLS & MASKS",
            Self::ViewAndOverlays => "VIEW & OVERLAYS",
            Self::CullingAndNav => "CULLING & NAVIGATION",
            Self::Workflow => "WORKFLOW & SESSIONS",
        }
    }
}

/// Customizable single-key actions persisted in `~/.config/rune-darkroom/settings.json`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShortcutAction {
    AiSelect,
    Brush,
    Eraser,
    LinearMask,
    RadialMask,
    Heal,
    Clone,
    Crop,
    RedMaskOverlay,
    Loupe,
    CompareSplit,
    GridBrowser,
    PickFlag,
    RejectFlag,
    Filmstrip,
    Inspector,
}

impl ShortcutAction {
    pub const ALL: [Self; 16] = [
        Self::AiSelect,
        Self::Brush,
        Self::Eraser,
        Self::LinearMask,
        Self::RadialMask,
        Self::Heal,
        Self::Clone,
        Self::Crop,
        Self::RedMaskOverlay,
        Self::Loupe,
        Self::CompareSplit,
        Self::GridBrowser,
        Self::PickFlag,
        Self::RejectFlag,
        Self::Filmstrip,
        Self::Inspector,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::AiSelect => "ai_select",
            Self::Brush => "brush",
            Self::Eraser => "eraser",
            Self::LinearMask => "linear_mask",
            Self::RadialMask => "radial_mask",
            Self::Heal => "heal",
            Self::Clone => "clone",
            Self::Crop => "crop",
            Self::RedMaskOverlay => "red_mask_overlay",
            Self::Loupe => "loupe",
            Self::CompareSplit => "compare_split",
            Self::GridBrowser => "grid_browser",
            Self::PickFlag => "pick_flag",
            Self::RejectFlag => "reject_flag",
            Self::Filmstrip => "filmstrip",
            Self::Inspector => "inspector",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::AiSelect => "AI Select Mask (Layers)",
            Self::Brush => "Mask Brush",
            Self::Eraser => "Mask Eraser (Layers)",
            Self::LinearMask => "Linear Gradient Mask",
            Self::RadialMask => "Radial Gradient Mask (Layers)",
            Self::Heal => "Spot Heal (Layers)",
            Self::Clone => "Clone Stamp (Layers)",
            Self::Crop => "Crop & Geometry",
            Self::RedMaskOverlay => "Red Mask Overlay",
            Self::Loupe => "GPU Loupe Magnifier",
            Self::CompareSplit => "Before / After Split",
            Self::GridBrowser => "Grid Browser",
            Self::PickFlag => "Flag as Pick",
            Self::RejectFlag => "Flag as Reject",
            Self::Filmstrip => "Toggle Filmstrip",
            Self::Inspector => "Toggle Inspector",
        }
    }

    pub const fn category(self) -> ShortcutCategory {
        match self {
            Self::AiSelect
            | Self::Brush
            | Self::Eraser
            | Self::LinearMask
            | Self::RadialMask
            | Self::Heal
            | Self::Clone
            | Self::Crop => ShortcutCategory::ToolsAndMasks,
            Self::RedMaskOverlay | Self::Loupe | Self::CompareSplit | Self::GridBrowser => {
                ShortcutCategory::ViewAndOverlays
            }
            Self::PickFlag | Self::RejectFlag | Self::Filmstrip | Self::Inspector => {
                ShortcutCategory::CullingAndNav
            }
        }
    }

    pub const fn default_key(self) -> &'static str {
        match self {
            Self::AiSelect => "w",
            Self::Brush => "b",
            Self::Eraser => "e",
            Self::LinearMask => "l",
            Self::RadialMask => "t",
            Self::Heal => "q",
            Self::Clone => "s",
            Self::Crop => "c",
            Self::RedMaskOverlay => "m",
            Self::Loupe => "o",
            Self::CompareSplit => "\\",
            Self::GridBrowser => "g",
            Self::PickFlag => "p",
            Self::RejectFlag => "x",
            Self::Filmstrip => "f",
            Self::Inspector => "i",
        }
    }
}

/// User-customizable shortcut binding table with safe validation & defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShortcutBindings {
    pub overrides: BTreeMap<String, String>,
}

impl ShortcutBindings {
    pub fn key_for(&self, action: ShortcutAction) -> &str {
        self.overrides
            .get(action.id())
            .map(String::as_str)
            .unwrap_or_else(|| action.default_key())
    }

    pub fn display_key(&self, action: ShortcutAction) -> String {
        let k = self.key_for(action);
        if k == "\\" {
            "\\".to_owned()
        } else {
            k.to_ascii_uppercase()
        }
    }

    pub fn matches(&self, action: ShortcutAction, ch: &str) -> bool {
        self.key_for(action).eq_ignore_ascii_case(ch)
    }

    pub fn is_customized(&self, action: ShortcutAction) -> bool {
        self.overrides
            .get(action.id())
            .is_some_and(|v| !v.eq_ignore_ascii_case(action.default_key()))
    }

    pub fn is_any_customized(&self) -> bool {
        ShortcutAction::ALL
            .into_iter()
            .any(|a| self.is_customized(a))
    }

    pub fn set_key(&mut self, action: ShortcutAction, raw_key: &str) -> Result<(), String> {
        let norm = raw_key.trim().to_ascii_lowercase();
        if norm.chars().count() != 1 {
            return Err("Shortcut must be a single character key".to_owned());
        }
        let c = norm.chars().next().unwrap();
        if matches!(c, '0'..='5') {
            return Err("Digits 0–5 are reserved for star ratings".to_owned());
        }
        if !(c.is_ascii_alphanumeric() || matches!(c, '\\' | '[' | ']' | '/' | '.' | ',' | ';')) {
            return Err(format!("Unsupported shortcut key '{c}'"));
        }
        if norm == action.default_key() {
            self.overrides.remove(action.id());
        } else {
            self.overrides.insert(action.id().to_owned(), norm);
        }
        Ok(())
    }

    pub fn reset_defaults(&mut self) {
        self.overrides.clear();
    }
}

/// Unified persisted settings written to `~/.config/rune-darkroom/settings.json`
/// (100% backward-compatible with `McpSettings` JSON schema).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedAppSettings {
    pub mcp: McpSettings,
    pub proof_margin: ProofMargin,
    pub hot_folder_style: Option<String>,
    pub shortcuts: ShortcutBindings,
}

impl Default for PersistedAppSettings {
    fn default() -> Self {
        Self {
            mcp: McpSettings::default(),
            proof_margin: ProofMargin::Compact,
            hot_folder_style: None,
            shortcuts: ShortcutBindings::default(),
        }
    }
}

fn escape_json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

fn unescape_json_str(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(esc) = chars.next() {
                match esc {
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    other => out.push(other),
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn extract_json_string_field(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let pos = json.find(&needle)?;
    let rest = &json[pos + needle.len()..];
    let colon = rest.find(':')?;
    let val = rest[colon + 1..].trim_start();
    if !val.starts_with('"') {
        return None;
    }
    let inner = &val[1..];
    let mut escaped = false;
    for (i, ch) in inner.char_indices() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(unescape_json_str(&inner[..i]));
        }
    }
    None
}

fn extract_json_object_strings(json: &str, key: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let needle = format!("\"{key}\"");
    let Some(pos) = json.find(&needle) else {
        return map;
    };
    let rest = &json[pos + needle.len()..];
    let Some(open) = rest.find('{') else {
        return map;
    };
    let body = &rest[open + 1..];
    let Some(close) = body.find('}') else {
        return map;
    };
    let obj = &body[..close];
    let mut idx = 0;
    let bytes = obj.as_bytes();
    while idx < bytes.len() {
        let Some(k_start) = obj[idx..].find('"') else {
            break;
        };
        let k_abs = idx + k_start + 1;
        let Some(k_end) = obj[k_abs..].find('"') else {
            break;
        };
        let k_str = unescape_json_str(&obj[k_abs..k_abs + k_end]);
        idx = k_abs + k_end + 1;
        let Some(colon) = obj[idx..].find(':') else {
            break;
        };
        idx += colon + 1;
        let val_slice = obj[idx..].trim_start();
        if !val_slice.starts_with('"') {
            break;
        }
        let v_rel_start = obj.len() - val_slice.len() + 1;
        let v_inner = &obj[v_rel_start..];
        let mut escaped = false;
        let mut v_end = None;
        for (i, ch) in v_inner.char_indices() {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                v_end = Some(i);
                break;
            }
        }
        let Some(ve) = v_end else {
            break;
        };
        let v_str = unescape_json_str(&v_inner[..ve]);
        map.insert(k_str, v_str);
        idx = v_rel_start + ve + 1;
    }
    map
}

impl PersistedAppSettings {
    pub fn to_json_pretty(&self) -> String {
        let mut out = String::from("{\n");
        out.push_str(&format!("  \"enabled\": {},\n", self.mcp.enabled));
        out.push_str(&format!("  \"port\": {},\n", self.mcp.port));
        out.push_str(&format!(
            "  \"allow_live_canvas_edits\": {},\n",
            self.mcp.allow_live_canvas_edits
        ));
        out.push_str(&format!(
            "  \"proof_margin\": \"{}\",\n",
            self.proof_margin.slug()
        ));
        if let Some(style) = self.hot_folder_style.as_deref() {
            out.push_str(&format!(
                "  \"hot_folder_style\": \"{}\",\n",
                escape_json_str(style)
            ));
        } else {
            out.push_str("  \"hot_folder_style\": null,\n");
        }
        out.push_str("  \"shortcuts\": {");
        if !self.shortcuts.overrides.is_empty() {
            out.push('\n');
            let len = self.shortcuts.overrides.len();
            for (i, (k, v)) in self.shortcuts.overrides.iter().enumerate() {
                let comma = if i + 1 < len { "," } else { "" };
                out.push_str(&format!(
                    "    \"{}\": \"{}\"{comma}\n",
                    escape_json_str(k),
                    escape_json_str(v)
                ));
            }
            out.push_str("  }\n");
        } else {
            out.push_str("}\n");
        }
        out.push_str("}\n");
        out
    }

    pub fn from_json_str(text: &str, mcp: McpSettings) -> Self {
        let proof_margin = extract_json_string_field(text, "proof_margin")
            .and_then(|s| ProofMargin::from_slug(&s))
            .unwrap_or_default();
        let hot_folder_style = extract_json_string_field(text, "hot_folder_style");
        let mut shortcuts = ShortcutBindings::default();
        for (k, v) in extract_json_object_strings(text, "shortcuts") {
            if let Some(action) = ShortcutAction::from_id(&k) {
                let _ = shortcuts.set_key(action, &v);
            }
        }
        Self {
            mcp,
            proof_margin,
            hot_folder_style,
            shortcuts,
        }
    }

    pub fn load() -> Self {
        let mcp = McpSettings::load();
        let Some(path) = McpSettings::config_path() else {
            return Self {
                mcp,
                ..Self::default()
            };
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self {
                mcp,
                ..Self::default()
            };
        };
        Self::from_json_str(&text, mcp)
    }

    pub fn save_to_path(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = self.to_json_pretty();
        rune_editor_core::session::atomic_write_bytes(path, json.as_bytes())
            .map_err(|e| e.to_string())
    }

    pub fn save(&self) -> Result<PathBuf, String> {
        let path = McpSettings::config_path()
            .ok_or_else(|| "Could not determine HOME directory for settings".to_owned())?;
        self.save_to_path(&path)?;
        Ok(path)
    }
}

/// Selected connection guide tab inside Settings → Agent & Local MCP.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum McpClientGuide {
    #[default]
    StreamableHttp,
    StdioCli,
    SkillMd,
}

/// Hover `ⓘ` Tier 2 info popover metadata (purely transient UI state — never dirties recipe
/// or pushes undo history).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InfoTip {
    pub title: &'static str,
    pub shortcut: Option<&'static str>,
    pub description: &'static str,
    pub cli_key: &'static str,
    pub tier: &'static str,
}

pub struct AppState {
    pub image: Arc<LinearImage>,
    /// Incremented whenever `image` changes so the GPU compositor re-uploads once.
    pub image_generation: u64,
    pub recipe: DevelopRecipe,
    /// Cached compiled recipe for O(1) live cursor color readout without per-move rebuilds.
    pub compiled_recipe: CompiledRecipe,
    /// Last persisted/baseline recipe for macOS `documentEdited` dirty tracking.
    pub saved_recipe: DevelopRecipe,
    pub history: HistoryStack,
    pub camera: ViewportCamera,
    pub histogram: HistogramData,
    pub presets: Vec<StylePreset>,

    // Inspector & workspace layout state
    pub active_tab: InspectorTab,
    pub curve_channel: CurveChannel,
    pub levels_channel: LevelsChannel,
    pub color_balance_view: ColorBalanceView,
    pub color_editor_mode: ColorEditorMode,
    pub selected_color_slice: usize,
    pub dragging_curve_pt: Option<usize>,
    pub selected_hsl_band: usize,
    pub show_inspector: bool,
    pub show_filmstrip: bool,
    pub base_open: bool,
    pub wb_open: bool,
    pub tone_open: bool,
    pub levels_open: bool,
    pub curve_open: bool,
    pub color_balance_open: bool,
    pub presence_open: bool,
    pub bw_open: bool,
    pub hsl_open: bool,
    pub clarity_open: bool,
    pub sharpen_open: bool,
    pub nr_open: bool,
    pub grain_open: bool,
    pub vignette_open: bool,
    pub crop_open: bool,
    pub rotate_open: bool,
    pub keystone_open: bool,
    pub lens_open: bool,
    pub grid_open: bool,
    pub layers_open: bool,
    pub masks_open: bool,
    pub local_adj_open: bool,
    pub heal_open: bool,

    // Studio Airy 3-tier progressive disclosure toggles (Tier 1 inline `Advanced ▸` drawers)
    pub profile_picker_open: bool,
    pub base_advanced_open: bool,
    pub wb_presets_open: bool,
    pub tone_advanced_open: bool,
    pub levels_advanced_open: bool,
    pub film_neg_advanced_open: bool,
    pub color_adv_range_open: bool,
    pub skin_tone_advanced_open: bool,
    pub bw_split_open: bool,
    pub clarity_advanced_open: bool,
    pub sharpen_advanced_open: bool,
    pub grain_advanced_open: bool,
    pub vignette_advanced_open: bool,
    pub crop_advanced_open: bool,
    pub keystone_advanced_open: bool,
    pub lens_advanced_open: bool,
    pub layers_mask_actions_open: bool,
    pub local_adj_advanced_open: bool,
    pub heal_advanced_open: bool,
    pub user_styles_advanced_open: bool,
    pub clipboard_groups_open: bool,

    // Cached user camera profiles (EXIF-matched sorted first; refreshed on photo open/import/picker open)
    pub user_profiles: Vec<(CameraProfileDescriptor, bool)>,
    pub active_raw_decode_mode: RawDecodeMode,

    // Studio Airy Settings progressive disclosure & transient floating popovers
    pub settings_mcp_snippets_open: bool,
    pub settings_mcp_tools_open: bool,
    pub settings_security_invariants_open: bool,
    pub overlays_popover_open: bool,
    pub hovered_info: Option<InfoTip>,

    // Interactive Capture One tools & live probe state
    pub active_tool: CanvasTool,
    pub active_layer_index: Option<usize>,
    pub current_mods: Mods,
    pub ai_select_points: Vec<PromptPoint>,
    pub ai_select_bbox: Option<PromptBox>,
    pub ai_select_bbox_preview: Option<PromptBox>,
    pub ai_select_invert: bool,
    pub ai_select_include_mode: bool,
    pub ai_select_target_layer: Option<usize>,
    pub last_ai_mask_status: Option<AiMaskStatus>,
    pub brush_radius: f32,
    pub brush_hardness: f32,
    pub brush_opacity: f32,
    pub heal_radius: f32,
    pub heal_feather: f32,
    pub heal_opacity: f32,
    pub layer_drag: Option<LayerDragState>,
    pub wb_eyedropper_active: bool,
    pub film_base_picker_active: bool,
    pub film_neg_open: bool,
    pub color_slice_picker_active: bool,
    pub skin_picker_active: bool,
    pub readout_picker_active: bool,
    pub pinned_readouts: Vec<PinnedReadout>,
    pub speed_edit: Option<SpeedEditParam>,
    pub speed_edit_snapshot: Option<DevelopRecipe>,
    pub viewport_rect_pts: [f32; 2],
    pub viewport_scale: f32,
    pub cursor_readout: Option<CursorColorReadout>,
    pub crop_drag: Option<(CropDragHandle, [f32; 2], [f32; 4])>,

    // Phase 3 Pro Workflow: Grid Browser, Auto-Advance, Compare Variant, Styles & Clipboard, Multi-Recipe Export
    pub auto_advance: bool,
    pub show_grid_browser: bool,
    pub browser_search_edit: TextEdit,
    pub compare_reference: Option<CompareReference>,
    pub clipboard_groups: HashSet<String>,
    pub clipboard_open: bool,
    pub variants_open: bool,
    pub styles_open: bool,
    pub user_styles: Vec<UserStyle>,
    pub user_styles_open: bool,
    pub style_name_edit: TextEdit,
    pub stack_styles: bool,
    pub style_strength: f32,
    pub show_export: bool,
    pub export_recipes: Vec<ExportRecipeSpec>,
    pub export_naming_edit: TextEdit,
    pub export_watermark_enabled: bool,
    pub export_watermark_edit: TextEdit,
    pub export_watermark_opacity: f32,
    pub export_watermark_position: WatermarkPosition,
    pub export_strip_metadata: bool,
    pub export_color_space: ExportColorSpace,
    pub export_output_dir: Option<PathBuf>,

    // Phase 5: Proof Margin, Hot Folder Auto-Ingest & Custom Keyboard Shortcuts
    pub proof_margin: ProofMargin,
    pub hot_folder_watcher: Option<HotFolderWatcher>,
    pub hot_folder_dir: Option<PathBuf>,
    pub hot_folder_style: Option<String>,
    pub hot_folder_ingested_count: usize,
    pub shortcuts: ShortcutBindings,
    pub recording_shortcut: Option<ShortcutAction>,

    // Folder / session catalog & filmstrip culling state
    pub catalog: SessionCatalog,
    pub demo_mode: bool,
    pub thumbnails: HashMap<PathBuf, Pixels>,
    pub prefetch_cache: Arc<Mutex<HashMap<PathBuf, Arc<LinearImage>>>>,
    pub prefetch_in_flight: Arc<Mutex<HashSet<PathBuf>>>,

    // Pointer / drag & drop interaction state
    pub drag_undo_snapshot: Option<DevelopRecipe>,
    pub last_drag_pos: Option<(f32, f32)>,
    pub drop_hover: bool,

    // Status, modal sheets, Settings view, Local MCP server & native menu handle
    pub status_message: Option<String>,
    pub show_about: bool,
    pub confirm_close: bool,
    pub show_settings: bool,
    pub settings_section: SettingsSection,
    pub mcp_client_guide: McpClientGuide,
    pub mcp_settings: McpSettings,
    pub mcp_server: Option<LocalMcpServer>,
    pub mcp_shared_canvas: Arc<Mutex<ActiveCanvasSnapshot>>,
    pub mcp_allow_live: Arc<AtomicBool>,
    pub mcp_error: Option<String>,
    pub mcp_request_count: u64,
    pub mcp_last_activity: Option<String>,
    pub mcp_copied_tag: Option<String>,
    pub app_handle: Option<AppHandle<AppState>>,

    // AI Segmentation Models & Download Manager state (Settings → AI Models)
    pub ai_model_config: AiModelConfig,
    pub ai_model_statuses: Vec<ModelStatus>,
    pub ai_engine_summary: ActiveEngineSummary,
    pub ai_license_toggles: HashMap<ModelId, bool>,
    pub ai_download_jobs: ModelDownloadRegistry,
    pub ai_download_revision: u64,
    /// Non-`None` while a background AI segmentation job is running off the UI thread.
    pub ai_segmenting_label: Option<String>,
    /// Monotonic token used to discard superseded background AI segmentation results.
    pub ai_seg_request_seq: Arc<AtomicU64>,
}

fn image_camera_make_model(image: &LinearImage) -> (Option<&str>, Option<&str>) {
    let make = image
        .camera_calibration
        .as_ref()
        .map(|c| c.make.as_str())
        .filter(|s| !s.trim().is_empty());
    let model = image
        .exif
        .as_ref()
        .and_then(|e| e.camera_model.as_deref())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            image
                .camera_calibration
                .as_ref()
                .map(|c| c.model.as_str())
                .filter(|s| !s.trim().is_empty())
        });
    (make, model)
}

fn collect_sorted_user_profiles(
    image: &LinearImage,
    profiles_dir: Option<&Path>,
) -> Vec<(CameraProfileDescriptor, bool)> {
    let (make, model) = image_camera_make_model(image);
    let all = list_all_camera_profiles(make, model, profiles_dir);
    let mut seen_builtin = false;
    let mut out = Vec::new();
    for desc in all {
        if desc.category == ProfileCategory::User {
            // `list_all_camera_profiles` places EXIF-matched user profiles strictly before builtins.
            out.push((desc, !seen_builtin));
        } else {
            seen_builtin = true;
        }
    }
    out
}

impl Default for AppState {
    fn default() -> Self {
        let image = Arc::new(
            load_from_memory(
                SAMPLE_PORTRAIT_BYTES,
                "Editorial_Portrait.jpg (Sample)",
                "Sample • 16-bit Linear FP16",
            )
            .unwrap_or_else(|_| generate_demo_hdr_scene(1600, 1066)),
        );
        let presets = builtin_presets();
        let recipe = DevelopRecipe::default();
        let compiled_recipe = CompiledRecipe::from_recipe(&recipe);
        let histogram = compute_histogram_compiled(&image.histogram_proxy, &compiled_recipe);

        let (catalog, thumbnails) = build_demo_session(&image, &presets);
        let persisted = PersistedAppSettings::load();
        let mcp_settings = persisted.mcp;
        let initial_snap = ActiveCanvasSnapshot {
            display_name: image.display_name.clone(),
            source_path: image.source_path.clone(),
            image: image.clone(),
            recipe: recipe.clone(),
            rating: catalog.active_meta().map_or(0, |m| m.rating),
            flag: catalog.active_meta().map_or(CullFlag::None, |m| m.flag),
            color_tag: catalog
                .active_entry()
                .map_or(ColorTag::None, |e| e.effective_color_tag()),
            demo_mode: true,
            folder: catalog.folder.clone(),
            catalog_len: catalog.len(),
        };
        let mcp_allow_live = Arc::new(AtomicBool::new(mcp_settings.allow_live_canvas_edits));

        let user_profiles = collect_sorted_user_profiles(&image, None);
        let ai_model_config = load_ai_model_config().unwrap_or_default();
        let ai_model_statuses = list_model_statuses();
        let ai_engine_summary = active_engine_summary();
        let mut ai_license_toggles = HashMap::new();
        for id in ModelId::ALL {
            ai_license_toggles.insert(id, ai_model_config.is_license_accepted(id));
        }

        Self {
            image,
            image_generation: 1,
            recipe: recipe.clone(),
            compiled_recipe,
            saved_recipe: recipe,
            history: HistoryStack::default(),
            camera: ViewportCamera::default(),
            histogram,
            presets,
            active_tab: InspectorTab::Light,
            curve_channel: CurveChannel::Luma,
            levels_channel: LevelsChannel::Rgb,
            color_balance_view: ColorBalanceView::ThreeWay,
            color_editor_mode: ColorEditorMode::Basic,
            selected_color_slice: 0,
            dragging_curve_pt: None,
            selected_hsl_band: 0,
            show_inspector: true,
            show_filmstrip: false,
            base_open: true,
            wb_open: true,
            tone_open: true,
            levels_open: false,
            curve_open: false,
            color_balance_open: true,
            presence_open: true,
            bw_open: false,
            hsl_open: false,
            clarity_open: true,
            sharpen_open: false,
            nr_open: false,
            grain_open: false,
            vignette_open: false,
            crop_open: true,
            rotate_open: true,
            keystone_open: false,
            lens_open: false,
            grid_open: false,
            layers_open: true,
            masks_open: false,
            local_adj_open: true,
            heal_open: false,
            profile_picker_open: false,
            base_advanced_open: false,
            wb_presets_open: false,
            tone_advanced_open: false,
            levels_advanced_open: false,
            film_neg_advanced_open: false,
            color_adv_range_open: false,
            skin_tone_advanced_open: false,
            bw_split_open: false,
            clarity_advanced_open: false,
            sharpen_advanced_open: false,
            grain_advanced_open: false,
            vignette_advanced_open: false,
            crop_advanced_open: false,
            keystone_advanced_open: false,
            lens_advanced_open: false,
            layers_mask_actions_open: false,
            local_adj_advanced_open: false,
            heal_advanced_open: false,
            user_styles_advanced_open: false,
            clipboard_groups_open: false,
            user_profiles,
            active_raw_decode_mode: RawDecodeMode::Linear,
            settings_mcp_snippets_open: false,
            settings_mcp_tools_open: false,
            settings_security_invariants_open: false,
            overlays_popover_open: false,
            hovered_info: None,
            active_tool: CanvasTool::Hand,
            active_layer_index: None,
            current_mods: Mods::default(),
            ai_select_points: Vec::new(),
            ai_select_bbox: None,
            ai_select_bbox_preview: None,
            ai_select_invert: false,
            ai_select_include_mode: true,
            ai_select_target_layer: None,
            last_ai_mask_status: None,
            brush_radius: 0.06,
            brush_hardness: 0.45,
            brush_opacity: 0.85,
            heal_radius: 0.032,
            heal_feather: 0.42,
            heal_opacity: 1.0,
            layer_drag: None,
            wb_eyedropper_active: false,
            film_base_picker_active: false,
            film_neg_open: true,
            color_slice_picker_active: false,
            skin_picker_active: false,
            readout_picker_active: false,
            pinned_readouts: Vec::new(),
            speed_edit: None,
            speed_edit_snapshot: None,
            viewport_rect_pts: [0.0, 0.0],
            viewport_scale: 1.0,
            cursor_readout: None,
            crop_drag: None,
            auto_advance: false,
            show_grid_browser: false,
            browser_search_edit: TextEdit::new(""),
            compare_reference: None,
            clipboard_groups: ALL_CLIPBOARD_GROUPS
                .iter()
                .filter(|(id, _)| *id != "geometry")
                .map(|(id, _)| (*id).to_owned())
                .collect(),
            clipboard_open: false,
            variants_open: false,
            styles_open: true,
            user_styles: list_user_styles().unwrap_or_default(),
            user_styles_open: false,
            style_name_edit: TextEdit::new(""),
            stack_styles: false,
            style_strength: 1.0,
            show_export: false,
            export_recipes: builtin_export_recipes(),
            export_naming_edit: TextEdit::new("{ImageName}_{Recipe}"),
            export_watermark_enabled: false,
            export_watermark_edit: TextEdit::new("© Rune Darkroom"),
            export_watermark_opacity: 0.45,
            export_watermark_position: WatermarkPosition::BottomRight,
            export_strip_metadata: false,
            export_color_space: ExportColorSpace::Srgb,
            export_output_dir: None,
            proof_margin: persisted.proof_margin,
            hot_folder_watcher: None,
            hot_folder_dir: None,
            hot_folder_style: persisted.hot_folder_style,
            hot_folder_ingested_count: 0,
            shortcuts: persisted.shortcuts,
            recording_shortcut: None,
            catalog,
            demo_mode: true,
            thumbnails,
            prefetch_cache: Arc::new(Mutex::new(HashMap::new())),
            prefetch_in_flight: Arc::new(Mutex::new(HashSet::new())),
            drag_undo_snapshot: None,
            last_drag_pos: None,
            drop_hover: false,
            status_message: Some("Ready — drop a RAW/photo or press ⌘O to open".to_owned()),
            show_about: false,
            confirm_close: false,
            show_settings: false,
            settings_section: SettingsSection::AgentMcp,
            mcp_client_guide: McpClientGuide::StreamableHttp,
            mcp_settings,
            mcp_server: None,
            mcp_shared_canvas: Arc::new(Mutex::new(initial_snap)),
            mcp_allow_live,
            mcp_error: None,
            mcp_request_count: 0,
            mcp_last_activity: None,
            mcp_copied_tag: None,
            app_handle: None,
            ai_model_config,
            ai_model_statuses,
            ai_engine_summary,
            ai_license_toggles,
            ai_download_jobs: ModelDownloadRegistry::default(),
            ai_download_revision: 0,
            ai_segmenting_label: None,
            ai_seg_request_seq: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl AppState {
    pub fn is_dirty(&self) -> bool {
        self.recipe != self.saved_recipe
    }

    pub fn any_text_input_focused(&self) -> bool {
        self.browser_search_edit.focused
            || self.style_name_edit.focused
            || self.export_naming_edit.focused
            || self.export_watermark_edit.focused
    }

    pub fn set_show_export_tx(&mut self, show: bool, tx: &mut Tx) {
        self.show_export = show;
        self.overlays_popover_open = false;
        self.hovered_info = None;
        if show {
            self.show_settings = false;
        }
        self.sync_window_state(tx);
    }

    /// Sets the transient Tier 2 hover `ⓘ` info popover without dirtying recipe or undo history.
    pub fn set_hovered_info(&mut self, tip: InfoTip) {
        self.hovered_info = Some(tip);
    }

    /// Clears the transient hover `ⓘ` info popover only if it still matches `title`.
    pub fn clear_hovered_info(&mut self, title: &'static str) {
        if self.hovered_info.is_some_and(|t| t.title == title) {
            self.hovered_info = None;
        }
    }

    /// Toggles the Concept A titlebar `Overlays ▾` floating diagnostic popover.
    pub fn toggle_overlays_popover(&mut self) {
        self.overlays_popover_open = !self.overlays_popover_open;
        if self.overlays_popover_open {
            self.hovered_info = None;
        }
    }

    /// Counts currently active diagnostic / guide overlays for the titlebar `Overlays ▾` indicator.
    pub fn active_overlay_count(&self) -> usize {
        let mut count = 0usize;
        if self.camera.exposure_warning {
            count += 1;
        }
        if self.camera.focus_mask {
            count += 1;
        }
        if self.camera.mask_overlay_mode != 0 {
            count += 1;
        }
        if self.camera.loupe_enabled {
            count += 1;
        }
        if self.camera.crop_mode
            && self.recipe.geometry.grid != rune_editor_core::CompositionGrid::None
        {
            count += 1;
        }
        count
    }

    pub fn toggle_grid_browser_tx(&mut self, tx: &mut Tx) {
        self.show_grid_browser = !self.show_grid_browser;
        if self.show_grid_browser {
            self.camera.crop_mode = false;
            self.camera.loupe_enabled = false;
            self.wb_eyedropper_active = false;
            self.color_slice_picker_active = false;
            self.skin_picker_active = false;
            self.readout_picker_active = false;
            self.status_message =
                Some("Grid Browser — click to select, press G for Viewer".to_owned());
        } else {
            self.status_message = Some("Single-Frame Viewer".to_owned());
        }
        self.sync_window_state(tx);
    }

    pub fn is_mcp_running(&self) -> bool {
        self.mcp_server.is_some()
    }

    /// Keeps the lock-protected MCP snapshot in sync with the active canvas without blocking UI.
    pub fn sync_mcp_canvas_snapshot(&self) {
        if let Ok(mut snap) = self.mcp_shared_canvas.lock() {
            snap.display_name = self.image.display_name.clone();
            snap.source_path = self.image.source_path.clone();
            snap.image = self.image.clone();
            snap.recipe = self.recipe.clone();
            snap.rating = self.catalog.active_meta().map_or(0, |m| m.rating);
            snap.flag = self
                .catalog
                .active_meta()
                .map_or(CullFlag::None, |m| m.flag);
            snap.color_tag = self
                .catalog
                .active_entry()
                .map_or(ColorTag::None, |e| e.effective_color_tag());
            snap.demo_mode = self.demo_mode;
            snap.folder = self.catalog.folder.clone();
            snap.catalog_len = self.catalog.len();
        }
    }

    /// Starts the embedded Streamable HTTP MCP server if persisted settings have `enabled: true`.
    pub fn init_mcp_if_enabled(&mut self) {
        if self.mcp_settings.enabled && self.mcp_server.is_none() {
            self.start_mcp_server();
        }
    }

    fn start_mcp_server(&mut self) {
        let Some(handle) = self.app_handle.clone() else {
            self.mcp_error = Some("AppHandle unavailable in headless mode".to_owned());
            return;
        };
        self.sync_mcp_canvas_snapshot();
        self.mcp_allow_live
            .store(self.mcp_settings.allow_live_canvas_edits, Ordering::SeqCst);
        let bridge = Arc::new(GuiMcpBridge {
            handle,
            shared_canvas: self.mcp_shared_canvas.clone(),
            allow_live_edits: self.mcp_allow_live.clone(),
        });
        let service = Arc::new(McpService::new(bridge));
        match LocalMcpServer::start(self.mcp_settings.port, service) {
            Ok(server) => {
                self.mcp_error = None;
                let url = server.endpoint_url();
                self.mcp_server = Some(server);
                self.status_message = Some(format!("Local MCP running at {url}"));
            }
            Err(err) => {
                self.mcp_server = None;
                self.mcp_error = Some(err.clone());
                self.status_message = Some(format!("MCP failed to start: {err}"));
            }
        }
    }

    fn stop_mcp_server(&mut self) {
        if let Some(mut srv) = self.mcp_server.take() {
            srv.stop();
        }
        self.mcp_error = None;
        self.status_message = Some("Local MCP server stopped".to_owned());
    }

    pub fn set_show_settings_tx(&mut self, show: bool, tx: &mut Tx) {
        self.show_settings = show;
        self.overlays_popover_open = false;
        self.hovered_info = None;
        if show {
            self.show_export = false;
            self.refresh_ai_models();
        }
        self.mcp_copied_tag = None;
        self.sync_window_state(tx);
    }

    /// Jumps straight from Layers / darkroom workspace into `Settings → AI Models`.
    pub fn open_ai_models_settings_tx(&mut self, tx: &mut Tx) {
        self.settings_section = SettingsSection::AiModels;
        self.refresh_ai_models();
        self.set_show_settings_tx(true, tx);
    }

    /// Reloads AI segmentation config, disk install statuses, license toggles, and engine summary.
    pub fn refresh_ai_models(&mut self) {
        self.ai_model_config = load_ai_model_config().unwrap_or_default();
        self.ai_model_statuses = list_model_statuses();
        self.ai_engine_summary = active_engine_summary();
        for id in ModelId::ALL {
            let entry = self.ai_license_toggles.entry(id).or_insert(false);
            if self.ai_model_config.is_license_accepted(id) {
                *entry = true;
            }
        }
    }

    pub fn refresh_ai_models_tx(&mut self, tx: &mut Tx) {
        self.refresh_ai_models();
        self.status_message = Some("Refreshed AI segmentation models".to_owned());
        self.sync_window_state(tx);
    }

    /// Selects and atomically persists the active segmentation engine (`ai_models.json`),
    /// merging against on-disk config under a lock so CLI/MCP changes are never clobbered.
    pub fn set_ai_engine(&mut self, engine: SegmentationEngine) -> Result<(), String> {
        let updated = update_ai_model_config(|cfg| {
            cfg.active_engine = engine;
        })
        .map_err(|e| e.to_string())?;
        self.ai_model_config = updated;
        self.ai_model_statuses = list_model_statuses();
        self.ai_engine_summary = active_engine_summary();
        self.status_message = Some(format!(
            "AI Segmentation Engine: {} ({})",
            engine.display_name(),
            self.ai_effective_engine_readout()
        ));
        Ok(())
    }

    pub fn set_ai_engine_tx(&mut self, engine: SegmentationEngine, tx: &mut Tx) {
        if let Err(err) = self.set_ai_engine(engine) {
            self.status_message = Some(format!("Failed to save AI model config: {err}"));
        }
        self.sync_window_state(tx);
    }

    /// Human-readable effective engine summary shown in `Settings → AI Models`.
    pub fn ai_effective_engine_readout(&self) -> String {
        let subj = match self.ai_engine_summary.effective_auto_engine {
            SegmentationEngine::AppleVision => "Apple Vision ANE",
            SegmentationEngine::Sam3Tracker => "SAM 3",
            SegmentationEngine::Sam21Tiny => "SAM 2.1",
            SegmentationEngine::Heuristic => "Heuristic",
            SegmentationEngine::Auto => "Auto",
        };
        let inter = match self.ai_engine_summary.effective_interactive_engine {
            SegmentationEngine::AppleVision => "Apple Vision ANE",
            SegmentationEngine::Sam3Tracker => "SAM 3",
            SegmentationEngine::Sam21Tiny => "SAM 2.1",
            SegmentationEngine::Heuristic => "Heuristic",
            SegmentationEngine::Auto => "Auto",
        };
        format!("Subject: {subj} · Interactive: {inter}")
    }

    pub fn is_ai_license_checked(&self, id: ModelId) -> bool {
        !id.spec().requires_license_acceptance
            || self.ai_model_config.is_license_accepted(id)
            || self.ai_license_toggles.get(&id).copied().unwrap_or(false)
    }

    pub fn toggle_ai_license_acceptance(&mut self, id: ModelId) {
        let next = !self.ai_license_toggles.get(&id).copied().unwrap_or(false);
        self.ai_license_toggles.insert(id, next);
        // If user accepts a SAM 3 license, mark both SAM 3 variants toggled so UI stays cohesive.
        if id.is_sam3() {
            self.ai_license_toggles
                .insert(ModelId::Sam3TrackerQ4f16, next);
            self.ai_license_toggles
                .insert(ModelId::Sam3TrackerFp16, next);
        }
    }

    /// Returns a snapshot of the current download job for `id`, if any.
    pub fn model_download_job(&self, id: ModelId) -> Option<ModelDownloadJob> {
        let job_arc = self
            .ai_download_jobs
            .jobs
            .lock()
            .ok()
            .and_then(|map| map.get(&id).cloned())?;
        job_arc.lock().ok().map(|j| j.clone())
    }

    /// Starts a background download for `id` using `download_model` with live progress & cancel.
    pub fn start_model_download(&mut self, id: ModelId) -> Result<(), String> {
        let spec = id.spec();
        let accepted = self.is_ai_license_checked(id);
        if spec.requires_license_acceptance && !accepted {
            let msg = format!(
                "Accept the {} before downloading {}",
                spec.license, spec.display_name
            );
            let job = Arc::new(Mutex::new(ModelDownloadJob::with_phase(
                ModelDownloadPhase::Error(msg.clone()),
            )));
            if let Ok(mut map) = self.ai_download_jobs.jobs.lock() {
                map.insert(id, job);
            }
            self.status_message = Some(msg.clone());
            return Err(msg);
        }

        if let Some(existing) = self.model_download_job(id)
            && existing.is_active()
        {
            return Ok(());
        }

        let initial_job = ModelDownloadJob::with_phase(ModelDownloadPhase::Downloading {
            fraction: 0.0,
            file_index: 0,
            total_files: spec.files.len(),
            file_name: spec
                .files
                .first()
                .map(|f| f.filename.to_owned())
                .unwrap_or_default(),
            bytes_downloaded: 0,
            total_bytes: spec.approx_bytes,
            status: format!("Starting {}…", spec.display_name),
        });
        let cancel = initial_job.cancel.clone();
        let job = Arc::new(Mutex::new(initial_job));
        if let Ok(mut map) = self.ai_download_jobs.jobs.lock() {
            map.insert(id, job.clone());
        }
        self.status_message = Some(format!("Downloading {}…", spec.display_name));

        let app_handle = self.app_handle.clone();
        std::thread::spawn(move || {
            let progress_job = job.clone();
            let progress_handle = app_handle.clone();
            let result = download_model(id, accepted, Some(cancel), move |progress| {
                if let Ok(mut guard) = progress_job.lock() {
                    guard.apply_progress(progress);
                }
                if let Some(ref handle) = progress_handle {
                    handle.update_if(|s, _| {
                        s.ai_download_revision = s.ai_download_revision.saturating_add(1);
                        true
                    });
                }
            });

            let final_status_msg = match result {
                Ok(_status) => {
                    if let Ok(mut guard) = job.lock() {
                        guard.phase = ModelDownloadPhase::Completed;
                    }
                    format!(
                        "Installed {} — ready for Smart Selection",
                        spec.display_name
                    )
                }
                Err(rune_editor_decode::ModelError::Cancelled(_)) => {
                    if let Ok(mut guard) = job.lock() {
                        guard.phase = ModelDownloadPhase::Cancelled;
                    }
                    format!("Cancelled download of {}", spec.display_name)
                }
                Err(err) => {
                    let msg = err.to_string();
                    if let Ok(mut guard) = job.lock() {
                        guard.phase = ModelDownloadPhase::Error(msg.clone());
                    }
                    format!("Download failed for {}: {msg}", spec.display_name)
                }
            };

            if let Some(handle) = app_handle {
                handle.update_if(move |s, _| {
                    s.ai_download_revision = s.ai_download_revision.saturating_add(1);
                    let completed = matches!(
                        s.model_download_job(id).map(|j| j.phase),
                        Some(ModelDownloadPhase::Completed)
                    );
                    if id.is_sam3() && completed {
                        s.persist_preferred_sam3_variant(id);
                    } else {
                        s.refresh_ai_models();
                    }
                    s.status_message = Some(final_status_msg);
                    true
                });
            }
        });

        Ok(())
    }

    /// Reloads `ai_models.json` from disk under a lock (preserving any `accepted_licenses` written
    /// by `download_model` / `setup_model_from_local_dir`), updates `preferred_sam3_variant`, and saves.
    fn persist_preferred_sam3_variant_at(&mut self, config_path: &Path, id: ModelId) {
        if !id.is_sam3() {
            return;
        }
        if let Ok(fresh) = update_ai_model_config_at(config_path, |cfg| {
            cfg.preferred_sam3_variant = id;
        }) {
            self.ai_model_config = fresh;
        }
    }

    fn persist_preferred_sam3_variant(&mut self, id: ModelId) {
        let path = rune_editor_core::ai_model_config_path();
        self.persist_preferred_sam3_variant_at(&path, id);
        self.refresh_ai_models();
    }

    /// Signals cancellation on every active background ONNX model download job (called on Drop/close).
    pub fn cancel_all_model_downloads(&self) {
        self.ai_download_jobs.cancel_all();
    }

    pub fn start_model_download_tx(&mut self, id: ModelId, tx: &mut Tx) {
        let _ = self.start_model_download(id);
        self.sync_window_state(tx);
    }

    /// Cancels an active download job for `id` via its shared `AtomicBool`.
    pub fn cancel_model_download(&mut self, id: ModelId) -> bool {
        let job_arc = self
            .ai_download_jobs
            .jobs
            .lock()
            .ok()
            .and_then(|map| map.get(&id).cloned());
        if let Some(job_arc) = job_arc
            && let Ok(job) = job_arc.lock()
        {
            job.request_cancel();
            self.status_message = Some(format!("Cancelling {} download…", id.display_name()));
            return true;
        }
        false
    }

    pub fn cancel_model_download_tx(&mut self, id: ModelId, tx: &mut Tx) {
        self.cancel_model_download(id);
        self.sync_window_state(tx);
    }

    /// Installs model `id` offline from a local directory using `setup_model_from_local_dir`.
    pub fn import_model_from_local_dir(&mut self, id: ModelId, dir: &Path) -> Result<(), String> {
        let accepted = self.is_ai_license_checked(id);
        match setup_model_from_local_dir(id, dir, accepted) {
            Ok(_) => {
                if let Ok(mut map) = self.ai_download_jobs.jobs.lock() {
                    map.remove(&id);
                }
                if id.is_sam3() {
                    self.persist_preferred_sam3_variant(id);
                } else {
                    self.refresh_ai_models();
                }
                self.status_message = Some(format!(
                    "Imported {} from {}",
                    id.display_name(),
                    dir.display()
                ));
                Ok(())
            }
            Err(err) => {
                let msg = err.to_string();
                let job = Arc::new(Mutex::new(ModelDownloadJob::with_phase(
                    ModelDownloadPhase::Error(msg.clone()),
                )));
                if let Ok(mut map) = self.ai_download_jobs.jobs.lock() {
                    map.insert(id, job);
                }
                self.status_message = Some(format!("Import failed: {msg}"));
                Err(msg)
            }
        }
    }

    pub fn import_model_from_local_dir_tx(&mut self, id: ModelId, dir: PathBuf, tx: &mut Tx) {
        if let Some(handle) = self.app_handle.clone() {
            let spec = id.spec();
            let accepted = self.is_ai_license_checked(id);
            if spec.requires_license_acceptance && !accepted {
                let _ = self.import_model_from_local_dir(id, &dir);
                self.sync_window_state(tx);
                return;
            }
            let job = Arc::new(Mutex::new(ModelDownloadJob::with_phase(
                ModelDownloadPhase::Downloading {
                    fraction: 0.25,
                    file_index: 0,
                    total_files: spec.files.len(),
                    file_name: "Copying local weights…".to_owned(),
                    bytes_downloaded: 0,
                    total_bytes: spec.approx_bytes,
                    status: format!("Importing {}…", spec.display_name),
                },
            )));
            if let Ok(mut map) = self.ai_download_jobs.jobs.lock() {
                map.insert(id, job.clone());
            }
            self.status_message = Some(format!("Importing {}…", spec.display_name));
            self.sync_window_state(tx);

            std::thread::spawn(move || {
                let result = setup_model_from_local_dir(id, &dir, accepted);
                handle.update_if(move |s, tx| {
                    s.ai_download_revision = s.ai_download_revision.saturating_add(1);
                    match result {
                        Ok(_) => {
                            if let Ok(mut map) = s.ai_download_jobs.jobs.lock() {
                                map.remove(&id);
                            }
                            if id.is_sam3() {
                                s.persist_preferred_sam3_variant(id);
                            } else {
                                s.refresh_ai_models();
                            }
                            s.status_message = Some(format!(
                                "Imported {} from {}",
                                id.display_name(),
                                dir.display()
                            ));
                        }
                        Err(err) => {
                            let msg = err.to_string();
                            if let Ok(mut guard) = job.lock() {
                                guard.phase = ModelDownloadPhase::Error(msg.clone());
                            }
                            s.status_message = Some(format!("Import failed: {msg}"));
                        }
                    }
                    s.sync_window_state(tx);
                    true
                });
            });
            return;
        }
        let _ = self.import_model_from_local_dir(id, &dir);
        self.sync_window_state(tx);
    }

    /// Removes installed/partial files for `id` from `models_root_dir()`.
    pub fn remove_ai_model(&mut self, id: ModelId) -> Result<(), String> {
        remove_model(id).map_err(|e| e.to_string())?;
        if let Ok(mut map) = self.ai_download_jobs.jobs.lock() {
            map.remove(&id);
        }
        self.refresh_ai_models();
        self.status_message = Some(format!("Removed {}", id.display_name()));
        Ok(())
    }

    pub fn remove_ai_model_tx(&mut self, id: ModelId, tx: &mut Tx) {
        if let Err(err) = self.remove_ai_model(id) {
            self.status_message = Some(format!("Failed to remove {}: {err}", id.display_name()));
        }
        self.sync_window_state(tx);
    }

    /// Opens `models_root_dir()` in Finder (macOS) or the OS file manager.
    pub fn open_ai_models_folder_tx(&mut self, tx: &mut Tx) {
        let dir = models_root_dir();
        let _ = std::fs::create_dir_all(&dir);
        #[cfg(target_os = "macos")]
        let res = Command::new("open").arg(&dir).spawn();
        #[cfg(target_os = "linux")]
        let res = Command::new("xdg-open").arg(&dir).spawn();
        #[cfg(target_os = "windows")]
        let res = Command::new("explorer").arg(&dir).spawn();

        match res {
            Ok(_) => {
                self.status_message = Some(format!("Opened models folder: {}", dir.display()));
            }
            Err(err) => {
                self.status_message = Some(format!("Could not open models folder: {err}"));
            }
        }
        self.sync_window_state(tx);
    }

    pub fn save_app_settings(&self) {
        let persisted = PersistedAppSettings {
            mcp: self.mcp_settings.clone(),
            proof_margin: self.proof_margin,
            hot_folder_style: self.hot_folder_style.clone(),
            shortcuts: self.shortcuts.clone(),
        };
        let _ = persisted.save();
    }

    pub fn toggle_mcp_server_tx(&mut self, tx: &mut Tx) {
        if self.mcp_server.is_some() {
            self.stop_mcp_server();
            self.mcp_settings.enabled = false;
        } else {
            self.mcp_settings.enabled = true;
            self.start_mcp_server();
            if self.mcp_server.is_none() {
                self.mcp_settings.enabled = false;
            }
        }
        self.save_app_settings();
        self.sync_window_state(tx);
    }

    pub fn toggle_mcp_live_edits_tx(&mut self, tx: &mut Tx) {
        self.mcp_settings.allow_live_canvas_edits = !self.mcp_settings.allow_live_canvas_edits;
        self.mcp_allow_live
            .store(self.mcp_settings.allow_live_canvas_edits, Ordering::SeqCst);
        self.save_app_settings();
        self.sync_window_state(tx);
    }

    pub fn set_mcp_port_tx(&mut self, port: u16, tx: &mut Tx) {
        if self.mcp_settings.port == port {
            return;
        }
        let was_running = self.mcp_server.is_some();
        if was_running {
            self.stop_mcp_server();
        }
        self.mcp_settings.port = port;
        if was_running {
            self.start_mcp_server();
            if self.mcp_server.is_none() {
                self.mcp_settings.enabled = false;
            }
        }
        self.save_app_settings();
        self.sync_window_state(tx);
    }

    pub fn apply_external_mcp_recipe(&mut self, new_recipe: DevelopRecipe) -> (bool, bool) {
        let sanitized = new_recipe.sanitized();
        let changed = self.recipe != sanitized;
        if changed {
            self.history.push(self.recipe.clone());
            self.recipe = sanitized;
            self.catalog.update_active_recipe(self.recipe.clone());
        }
        let mut meta_synced = false;
        if let Some(path) = self.image.source_path.as_deref() {
            if let Ok(Some(disk_meta)) = PhotoMeta::load_sidecar(path)
                && let Some(entry) = self.catalog.active_entry_mut()
            {
                entry.meta.revision = disk_meta.revision;
                entry.meta.history = disk_meta.history;
                entry.meta.rating = disk_meta.rating;
                entry.meta.flag = disk_meta.flag;
                entry.meta.color_tag = disk_meta.color_tag;
                entry.color_tag = disk_meta.color_tag;
                entry.meta.active_variant_index = disk_meta.active_variant_index;
                entry.meta.variants = disk_meta.variants;
                entry.meta.recipe = self.recipe.clone();
                meta_synced = true;
            }
            self.saved_recipe = self.recipe.clone();
        }
        if changed {
            self.sync_profile_state_after_recipe_change();
            self.recompute_histogram();
        }
        (changed, meta_synced)
    }

    /// Applies an edit delivered by a connected local MCP agent onto the live canvas,
    /// pushing the previous recipe onto `HistoryStack` so the user can hit `⌘Z` to undo.
    pub fn apply_external_mcp_recipe_tx(
        &mut self,
        new_recipe: DevelopRecipe,
        summary: String,
        tx: &mut Tx,
    ) {
        let (changed, meta_synced) = self.apply_external_mcp_recipe(new_recipe);
        if changed {
            self.status_message = Some(format!("{summary} (⌘Z to undo)"));
            self.sync_window_state(tx);
            tx.announce("Applied live MCP agent edit", LiveRegion::Polite);
        } else if meta_synced {
            self.status_message = Some(summary);
            self.sync_window_state(tx);
        }
    }

    pub fn recompute_histogram(&mut self) {
        if self
            .active_layer_index
            .is_some_and(|i| i >= self.recipe.layers.len())
        {
            self.active_layer_index = self.recipe.layers.len().checked_sub(1);
        }
        if self
            .ai_select_target_layer
            .is_some_and(|i| i >= self.recipe.layers.len())
        {
            self.ai_select_target_layer = None;
        }
        self.sync_active_layer_camera();
        self.compiled_recipe = CompiledRecipe::from_recipe(&self.recipe);
        self.histogram =
            compute_histogram_compiled(&self.image.histogram_proxy, &self.compiled_recipe);
        self.sync_mcp_canvas_snapshot();
    }

    /// Clears in-flight pointer/keyboard interaction state so menu actions, undo/redo,
    /// or photo switches never leak stale undo snapshots or stuck drags.
    pub fn clear_transient_interactions(&mut self) {
        self.speed_edit = None;
        self.speed_edit_snapshot = None;
        self.crop_drag = None;
        self.layer_drag = None;
        self.ai_select_bbox_preview = None;
        self.drag_undo_snapshot = None;
        self.dragging_curve_pt = None;
        self.last_drag_pos = None;
    }

    /// Syncs macOS `NSWindow.documentEdited`, live native menu enabled/checked states,
    /// and the shared MCP canvas snapshot.
    pub fn sync_window_state(&self, tx: &mut Tx) {
        self.sync_mcp_canvas_snapshot();
        tx.set_document_edited(self.is_dirty());
        self.sync_native_menu();
    }

    pub fn sync_native_menu(&self) {
        if let Some(handle) = &self.app_handle {
            let _ = handle.set_menu(build_menu_items(self));
        }
    }

    fn update_slider_inner(
        &mut self,
        done: bool,
        refresh_histogram: bool,
        mutate: impl FnOnce(&mut DevelopRecipe),
    ) {
        if self.drag_undo_snapshot.is_none() {
            self.drag_undo_snapshot = Some(self.recipe.clone());
        }
        mutate(&mut self.recipe);
        self.recipe.preset_name = None;
        if refresh_histogram {
            self.recompute_histogram();
        }
        if done {
            self.catalog.update_active_recipe(self.recipe.clone());
            if let Some(prev) = self.drag_undo_snapshot.take()
                && prev != self.recipe
            {
                self.history.push(prev);
            }
        }
    }

    pub fn update_slider_tx(
        &mut self,
        done: bool,
        tx: &mut Tx,
        mutate: impl FnOnce(&mut DevelopRecipe),
    ) {
        self.update_slider_inner(done, true, mutate);
        if done {
            self.sync_window_state(tx);
        }
    }

    /// Geometry-only slider update (skips histogram recomputation for 120Hz zero-lag drag).
    pub fn update_geometry_slider_tx(
        &mut self,
        done: bool,
        tx: &mut Tx,
        mutate: impl FnOnce(&mut DevelopRecipe),
    ) {
        self.update_slider_inner(done, false, mutate);
        if done {
            self.sync_window_state(tx);
        }
    }

    pub fn commit_recipe_change(&mut self, next: DevelopRecipe, status: impl Into<String>) {
        self.clear_transient_interactions();
        if self.recipe != next {
            self.history.push(self.recipe.clone());
            self.recipe = next;
            self.catalog.update_active_recipe(self.recipe.clone());
            self.sync_profile_state_after_recipe_change();
            self.recompute_histogram();
            self.status_message = Some(status.into());
        }
    }

    // -------------------------------------------------------------------------
    // Speed Edit (Capture One key+drag / key+scroll workflow)
    // -------------------------------------------------------------------------

    pub fn begin_speed_edit(&mut self, param: SpeedEditParam) {
        if self.speed_edit != Some(param) {
            self.speed_edit = Some(param);
        }
    }

    pub fn scrub_speed_edit(&mut self, delta: f32) -> bool {
        let Some(param) = self.speed_edit else {
            return false;
        };
        if self.speed_edit_snapshot.is_none() {
            self.speed_edit_snapshot = Some(self.recipe.clone());
        }
        if param.apply_delta(&mut self.recipe, delta) {
            self.recipe.preset_name = None;
            self.recompute_histogram();
            true
        } else {
            false
        }
    }

    pub fn end_speed_edit_tx(&mut self, tx: &mut Tx) {
        self.speed_edit = None;
        self.last_drag_pos = None;
        if let Some(prev) = self.speed_edit_snapshot.take()
            && prev != self.recipe
        {
            self.history.push(prev);
            self.catalog.update_active_recipe(self.recipe.clone());
            self.sync_window_state(tx);
        }
    }

    // -------------------------------------------------------------------------
    // Auto Color Science, WB Eyedropper & Viewer Diagnostics
    // -------------------------------------------------------------------------

    pub fn raw_decode_mode_for_recipe(recipe: &DevelopRecipe) -> RawDecodeMode {
        if canonical_profile_id(&recipe.camera_profile) == "apple_raw" {
            RawDecodeMode::AppleNativeBoost
        } else {
            RawDecodeMode::Linear
        }
    }

    pub fn refresh_user_profiles(&mut self) {
        self.user_profiles = collect_sorted_user_profiles(&self.image, None);
    }

    #[cfg(test)]
    pub fn refresh_user_profiles_in_dir(&mut self, dir: &Path) {
        self.user_profiles = collect_sorted_user_profiles(&self.image, Some(dir));
    }

    /// Returns `(display_label, is_exif_matched_user_profile)` for the active camera profile.
    pub fn active_camera_profile_label(&self) -> (String, bool) {
        let canon = canonical_profile_id(&self.recipe.camera_profile);
        if let Some((desc, matched)) = self.user_profiles.iter().find(|(d, _)| d.id == canon) {
            return (desc.name.clone(), *matched);
        }
        if let Some(desc) = find_builtin_profile(&canon) {
            return (desc.name, false);
        }
        let fallback = canon.strip_prefix("user:").unwrap_or(&canon);
        (fallback.replace('_', " "), false)
    }

    pub fn toggle_profile_picker(&mut self) {
        self.profile_picker_open = !self.profile_picker_open;
        if self.profile_picker_open {
            self.refresh_user_profiles();
        }
    }

    fn sync_profile_state_after_recipe_change(&mut self) {
        let canon = canonical_profile_id(&self.recipe.camera_profile);
        if canon.starts_with("user:") && !self.user_profiles.iter().any(|(d, _)| d.id == canon) {
            self.refresh_user_profiles();
        }
        self.maybe_redecode_active_raw_for_profile();
    }

    fn maybe_redecode_active_raw_for_profile(&mut self) {
        let desired_mode = Self::raw_decode_mode_for_recipe(&self.recipe);
        if desired_mode == self.active_raw_decode_mode {
            return;
        }
        let Some(path) = self.image.source_path.clone() else {
            self.active_raw_decode_mode = desired_mode;
            return;
        };
        if !is_raw_extension(&path) {
            self.active_raw_decode_mode = desired_mode;
            return;
        }
        if let Ok(decoded) = load_file_proxy_with_mode(&path, None, desired_mode) {
            self.image = Arc::new(decoded);
            self.image_generation = self.image_generation.wrapping_add(1);
            self.active_raw_decode_mode = desired_mode;
            self.refresh_user_profiles();
        }
    }

    pub fn set_camera_profile(&mut self, profile_id: impl AsRef<str>) {
        let canon = canonical_profile_id(profile_id.as_ref());
        if !is_valid_camera_profile_id(&canon) {
            self.status_message = Some(format!(
                "Unknown camera profile: {}",
                profile_id.as_ref().trim()
            ));
            return;
        }
        if canonical_profile_id(&self.recipe.camera_profile) == canon {
            return;
        }
        let mut next = self.recipe.clone();
        next.camera_profile = canon.clone();
        next.preset_name = None;
        let label = self
            .user_profiles
            .iter()
            .find(|(d, _)| d.id == canon)
            .map(|(d, _)| d.name.clone())
            .or_else(|| find_builtin_profile(&canon).map(|d| d.name))
            .unwrap_or(canon);
        self.commit_recipe_change(next, format!("Camera Profile: {label}"));
    }

    pub fn set_camera_profile_tx(&mut self, profile_id: impl AsRef<str>, tx: &mut Tx) {
        self.set_camera_profile(profile_id);
        self.sync_window_state(tx);
    }

    pub fn reset_base_characteristics_tx(&mut self, tx: &mut Tx) {
        let mut next = self.recipe.clone();
        next.camera_profile = DEFAULT_CAMERA_PROFILE.to_owned();
        next.profile_strength = DEFAULT_PROFILE_STRENGTH;
        next.film_curve = FilmCurve::FilmStandard;
        next.processing_mode = ProcessingMode::Photography;
        next.preset_name = None;
        self.commit_recipe_change(next, "Reset Base Characteristics");
        self.sync_window_state(tx);
    }

    pub fn import_camera_profile_path_tx(&mut self, path: PathBuf, tx: &mut Tx) {
        match import_user_profile(&path, None) {
            Ok(desc) => {
                self.refresh_user_profiles();
                self.profile_picker_open = false;
                let name = desc.name.clone();
                self.set_camera_profile(&desc.id);
                let msg = format!("Imported & activated profile: {name}");
                self.status_message = Some(msg.clone());
                self.sync_window_state(tx);
                tx.announce(msg, LiveRegion::Polite);
            }
            Err(err) => {
                self.status_message = Some(format!("Profile import failed: {err}"));
            }
        }
    }

    /// Hermetic profile import helper used by unit tests (writes into `dir` rather than `~/.config`).
    #[cfg(test)]
    pub fn import_camera_profile_from_dir(
        &mut self,
        dir: &Path,
        path: &Path,
    ) -> Result<CameraProfileDescriptor, String> {
        let desc = rune_editor_core::import_user_profile_in_dir(dir, path, None)
            .map_err(|e| e.to_string())?;
        self.refresh_user_profiles_in_dir(dir);
        self.profile_picker_open = false;
        self.set_camera_profile(&desc.id);
        self.status_message = Some(format!("Imported & activated profile: {}", desc.name));
        Ok(desc)
    }

    pub fn set_film_curve(&mut self, curve: FilmCurve) {
        if self.recipe.film_curve == curve {
            return;
        }
        let mut next = self.recipe.clone();
        next.film_curve = curve;
        next.preset_name = None;
        self.commit_recipe_change(next, format!("Film Curve: {}", curve.label()));
    }

    pub fn set_film_curve_tx(&mut self, curve: FilmCurve, tx: &mut Tx) {
        self.set_film_curve(curve);
        self.sync_window_state(tx);
    }

    pub fn apply_wb_preset(&mut self, preset: WbPreset) {
        let mut next = self.recipe.clone();
        next.apply_wb_preset(preset);
        next.preset_name = None;
        let kelvin = temperature_to_kelvin(next.temperature);
        self.commit_recipe_change(next, format!("WB Preset: {} ({kelvin} K)", preset.label()));
    }

    pub fn apply_wb_preset_tx(&mut self, preset: WbPreset, tx: &mut Tx) {
        self.apply_wb_preset(preset);
        self.sync_window_state(tx);
    }

    pub fn set_clarity_method(&mut self, method: ClarityMethod) {
        if self.recipe.clarity_method == method {
            return;
        }
        let mut next = self.recipe.clone();
        next.clarity_method = method;
        next.preset_name = None;
        self.commit_recipe_change(next, format!("Clarity Method: {}", method.label()));
    }

    pub fn set_clarity_method_tx(&mut self, method: ClarityMethod, tx: &mut Tx) {
        self.set_clarity_method(method);
        self.sync_window_state(tx);
    }

    pub fn set_grain_type(&mut self, grain_type: GrainType) {
        if self.recipe.grain_type == grain_type {
            return;
        }
        let mut next = self.recipe.clone();
        next.grain_type = grain_type;
        next.preset_name = None;
        self.commit_recipe_change(next, format!("Film Grain: {}", grain_type.label()));
    }

    pub fn set_grain_type_tx(&mut self, grain_type: GrainType, tx: &mut Tx) {
        self.set_grain_type(grain_type);
        self.sync_window_state(tx);
    }

    pub fn set_vignette_method(&mut self, method: VignetteMethod) {
        if self.recipe.vignette_method == method {
            return;
        }
        let mut next = self.recipe.clone();
        next.vignette_method = method;
        next.preset_name = None;
        self.commit_recipe_change(next, format!("Vignette Method: {}", method.label()));
    }

    pub fn set_vignette_method_tx(&mut self, method: VignetteMethod, tx: &mut Tx) {
        self.set_vignette_method(method);
        self.sync_window_state(tx);
    }

    pub fn set_active_tab(&mut self, tab: InspectorTab) {
        self.active_tab = tab;
        if tab != InspectorTab::Color {
            self.camera.view_color_range = -1;
            self.color_slice_picker_active = false;
            self.skin_picker_active = false;
        }
    }

    pub fn set_color_editor_mode(&mut self, mode: ColorEditorMode) {
        if self.color_editor_mode != mode {
            self.color_editor_mode = mode;
            self.camera.view_color_range = -1;
            if mode != ColorEditorMode::Advanced {
                self.color_slice_picker_active = false;
            }
            if mode != ColorEditorMode::SkinTone {
                self.skin_picker_active = false;
            }
        }
    }

    pub fn set_selected_color_slice(&mut self, index: usize) {
        let clamped = index.min(ADVANCED_COLOR_SLICE_COUNT - 1);
        self.selected_color_slice = clamped;
        if self.camera.view_color_range >= 0
            && (self.camera.view_color_range as usize) < ADVANCED_COLOR_SLICE_COUNT
        {
            self.camera.view_color_range = clamped as i32;
        }
    }

    pub fn toggle_view_color_range(&mut self, target: i32) {
        if self.camera.view_color_range == target {
            self.camera.view_color_range = -1;
            self.status_message = Some("View Selected Color Range OFF".to_owned());
        } else {
            self.camera.view_color_range = target;
            self.status_message = Some(if target == 4 {
                "Viewing Skin Tone selected color range".to_owned()
            } else {
                format!("Viewing Color Slice {} selected range", target + 1)
            });
        }
    }

    pub fn toggle_color_slice_picker_tx(&mut self, tx: &mut Tx) {
        self.color_slice_picker_active = !self.color_slice_picker_active;
        if self.color_slice_picker_active {
            self.wb_eyedropper_active = false;
            self.skin_picker_active = false;
            self.camera.crop_mode = false;
            self.crop_drag = None;
            self.active_tab = InspectorTab::Color;
            self.hsl_open = true;
            self.color_editor_mode = ColorEditorMode::Advanced;
            self.status_message = Some(format!(
                "Color Picker — click a color on the photo for Slice {}",
                self.selected_color_slice + 1
            ));
        } else {
            self.status_message = Some("Cancelled Color Picker".to_owned());
        }
        self.sync_window_state(tx);
    }

    pub fn toggle_skin_picker_tx(&mut self, tx: &mut Tx) {
        self.skin_picker_active = !self.skin_picker_active;
        if self.skin_picker_active {
            self.wb_eyedropper_active = false;
            self.color_slice_picker_active = false;
            self.camera.crop_mode = false;
            self.crop_drag = None;
            self.active_tab = InspectorTab::Color;
            self.hsl_open = true;
            self.color_editor_mode = ColorEditorMode::SkinTone;
            self.status_message =
                Some("Skin Tone Picker — click skin on the photo to sample target".to_owned());
        } else {
            self.status_message = Some("Cancelled Skin Tone Picker".to_owned());
        }
        self.sync_window_state(tx);
    }

    /// Samples a 3×3 linear RGB patch under the viewport click, populates the active
    /// Advanced Color Editor slice's target Hue/Sat, and enables the slice.
    pub fn pick_color_slice_at_viewport(&mut self, local_x: f32, local_y: f32) -> bool {
        let Some([su, sv]) = self.viewport_local_to_source_uv(local_x, local_y) else {
            self.color_slice_picker_active = false;
            self.status_message = Some("Click inside the photo to pick a color slice".to_owned());
            return false;
        };
        let avg = self.sample_raw_patch_linear(su, sv, 1);
        let pre_adv = self.compiled_recipe.evaluate_pre_advanced_color_linear(avg);
        let idx = self
            .selected_color_slice
            .min(ADVANCED_COLOR_SLICE_COUNT - 1);
        let mut next = self.recipe.clone();
        if let Some((hue, sat, _)) = next.pick_color_slice_from_rgb(idx, pre_adv) {
            next.preset_name = None;
            self.color_slice_picker_active = false;
            self.commit_recipe_change(
                next,
                format!(
                    "Picked Slice {} ({hue:.0}° · Sat {:.0}%)",
                    idx + 1,
                    sat * 100.0
                ),
            );
            true
        } else {
            self.color_slice_picker_active = false;
            false
        }
    }

    pub fn pick_color_slice_at_viewport_tx(&mut self, local_x: f32, local_y: f32, tx: &mut Tx) {
        if self.pick_color_slice_at_viewport(local_x, local_y) {
            self.sync_window_state(tx);
        }
    }

    /// Samples a 3×3 linear RGB patch under the viewport click, populates Skin Tone
    /// target H/S/L, and enables Skin Tone Uniformity.
    pub fn pick_skin_tone_at_viewport(&mut self, local_x: f32, local_y: f32) -> bool {
        let Some([su, sv]) = self.viewport_local_to_source_uv(local_x, local_y) else {
            self.skin_picker_active = false;
            self.status_message = Some("Click inside the photo to pick a skin tone".to_owned());
            return false;
        };
        let avg = self.sample_raw_patch_linear(su, sv, 1);
        let pre_adv = self.compiled_recipe.evaluate_pre_advanced_color_linear(avg);
        let mut next = self.recipe.clone();
        let (hue, sat, lum) = next.pick_skin_tone_from_rgb(pre_adv);
        next.preset_name = None;
        self.skin_picker_active = false;
        self.commit_recipe_change(
            next,
            format!(
                "Picked Skin Tone ({hue:.0}° · S{:.0} L{:.0})",
                sat * 100.0,
                lum * 100.0
            ),
        );
        true
    }

    pub fn pick_skin_tone_at_viewport_tx(&mut self, local_x: f32, local_y: f32, tx: &mut Tx) {
        if self.pick_skin_tone_at_viewport(local_x, local_y) {
            self.sync_window_state(tx);
        }
    }

    pub fn auto_white_balance_tx(&mut self, tx: &mut Tx) {
        let (temp, tint) = auto_white_balance(&self.image.histogram_proxy);
        let mut next = self.recipe.clone();
        next.temperature = temp;
        next.tint = tint;
        next.wb_preset = WbPreset::Custom;
        next.preset_name = None;
        let kelvin = temperature_to_kelvin(temp);
        let msg = format!("Auto WB ({kelvin} K, Tint {:+.0})", tint * 100.0);
        self.commit_recipe_change(next, msg);
        self.sync_window_state(tx);
    }

    pub fn auto_tone_tx(&mut self, tx: &mut Tx) {
        let (exp, con, hi, sh) = auto_exposure_and_tone(&self.image.histogram_proxy);
        let mut next = self.recipe.clone();
        next.exposure = exp;
        next.contrast = con;
        next.highlights = hi;
        next.shadows = sh;
        next.preset_name = None;
        self.commit_recipe_change(
            next,
            format!("Auto Tone (Exp {exp:+.2} EV, Con {:+.0})", con * 100.0),
        );
        self.sync_window_state(tx);
    }

    pub fn auto_levels_tx(&mut self, tx: &mut Tx) {
        let levels = auto_levels(&self.image.histogram_proxy);
        let mut next = self.recipe.clone();
        next.levels = levels;
        next.preset_name = None;
        self.commit_recipe_change(
            next,
            format!(
                "Auto Levels (In {:.0}–{:.0})",
                levels.in_black * 255.0,
                levels.in_white * 255.0
            ),
        );
        self.sync_window_state(tx);
    }

    pub fn toggle_exposure_warning_tx(&mut self, tx: &mut Tx) {
        self.camera.exposure_warning = !self.camera.exposure_warning;
        self.status_message = Some(if self.camera.exposure_warning {
            "Exposure & Shadow Clipping Warning ON".to_owned()
        } else {
            "Exposure Clipping Warning OFF".to_owned()
        });
        self.sync_window_state(tx);
    }

    pub fn toggle_focus_mask_tx(&mut self, tx: &mut Tx) {
        self.camera.focus_mask = !self.camera.focus_mask;
        self.status_message = Some(if self.camera.focus_mask {
            "Focus Mask Overlay ON".to_owned()
        } else {
            "Focus Mask Overlay OFF".to_owned()
        });
        self.sync_window_state(tx);
    }

    pub fn toggle_crop_mode_tx(&mut self, tx: &mut Tx) {
        self.camera.crop_mode = !self.camera.crop_mode;
        if self.camera.crop_mode {
            self.wb_eyedropper_active = false;
            self.color_slice_picker_active = false;
            self.skin_picker_active = false;
            self.camera.view_color_range = -1;
            self.camera.split_compare = false;
            self.active_tab = InspectorTab::Crop;
            self.crop_open = true;
            self.status_message =
                Some("Crop mode — drag corners/edges or pick an aspect ratio".to_owned());
        } else {
            self.crop_drag = None;
            self.status_message = Some("Applied crop preview".to_owned());
        }
        self.sync_window_state(tx);
    }

    pub fn toggle_wb_eyedropper_tx(&mut self, tx: &mut Tx) {
        self.wb_eyedropper_active = !self.wb_eyedropper_active;
        if self.wb_eyedropper_active {
            self.color_slice_picker_active = false;
            self.skin_picker_active = false;
            self.camera.view_color_range = -1;
            self.camera.crop_mode = false;
            self.crop_drag = None;
            self.active_tab = InspectorTab::Light;
            self.wb_open = true;
            self.status_message =
                Some("WB Eyedropper — click a neutral gray surface on the photo".to_owned());
        } else {
            self.status_message = Some("Cancelled WB Eyedropper".to_owned());
        }
        self.sync_window_state(tx);
    }

    /// Samples a 3×3 raw linear RGB patch under the viewport click and neutralizes WB.
    pub fn pick_wb_at_viewport_tx(&mut self, local_x: f32, local_y: f32, tx: &mut Tx) {
        let Some([su, sv]) = self.viewport_local_to_source_uv(local_x, local_y) else {
            self.wb_eyedropper_active = false;
            self.status_message = Some("Click inside the photo to pick White Balance".to_owned());
            return;
        };
        let avg = self.sample_raw_patch_linear(su, sv, 1);
        let (temp, tint) = pick_white_balance(avg);
        let mut next = self.recipe.clone();
        next.temperature = temp;
        next.tint = tint;
        next.wb_preset = WbPreset::Custom;
        next.preset_name = None;
        self.wb_eyedropper_active = false;
        let kelvin = temperature_to_kelvin(temp);
        self.commit_recipe_change(
            next,
            format!("Picked WB ({kelvin} K, Tint {:+.0})", tint * 100.0),
        );
        self.sync_window_state(tx);
    }

    /// O(1) live cursor color probe (`RGB • HSL • Lab`). Never recomputes the histogram.
    pub fn update_cursor_readout(&mut self, local_x: f32, local_y: f32) {
        let w = self.viewport_rect_pts[0];
        let h = self.viewport_rect_pts[1];
        if w <= 1.0 || h <= 1.0 {
            self.cursor_readout = None;
            return;
        }
        let slot_u = (local_x / w).clamp(0.0, 1.0);
        let is_before = self.camera.show_original
            || (self.camera.split_compare && slot_u < self.camera.split_pos);
        let distortion = if is_before {
            0.0
        } else {
            self.recipe.lens.sanitized().distortion
        };
        let Some([su, sv]) =
            self.viewport_local_to_source_uv_with_distortion(local_x, local_y, distortion)
        else {
            self.cursor_readout = None;
            return;
        };
        let src_lin = self.sample_raw_pixel_linear(su, sv);
        let graded_lin = if is_before {
            filmic_tonemap(src_lin)
        } else {
            self.compiled_recipe.evaluate_pixel_linear(src_lin)
        };
        let srgb = [
            linear_to_srgb(graded_lin[0]),
            linear_to_srgb(graded_lin[1]),
            linear_to_srgb(graded_lin[2]),
        ];
        let rgb8 = [
            (srgb[0] * 255.0).round().clamp(0.0, 255.0) as u8,
            (srgb[1] * 255.0).round().clamp(0.0, 255.0) as u8,
            (srgb[2] * 255.0).round().clamp(0.0, 255.0) as u8,
        ];
        let (hsl, lab) = rgb_to_hsl_and_lab(srgb);
        self.cursor_readout = Some(CursorColorReadout { rgb8, hsl, lab });
    }

    fn viewport_local_to_source_uv(&self, local_x: f32, local_y: f32) -> Option<[f32; 2]> {
        self.viewport_local_to_source_uv_with_distortion(
            local_x,
            local_y,
            self.recipe.lens.sanitized().distortion,
        )
    }

    fn viewport_local_to_source_uv_with_distortion(
        &self,
        local_x: f32,
        local_y: f32,
        distortion: f32,
    ) -> Option<[f32; 2]> {
        let w = self.viewport_rect_pts[0];
        let h = self.viewport_rect_pts[1];
        if w <= 1.0 || h <= 1.0 {
            return None;
        }
        let scale = self.viewport_scale.max(1.0);
        let slot_uv = [local_x / w, local_y / h];
        let slot_size = [w * scale, h * scale];
        self.recipe.geometry.viewport_to_source_uv_with_lens(
            slot_uv,
            slot_size,
            [self.image.width, self.image.height],
            &self.camera,
            distortion,
        )
    }

    fn sample_raw_pixel_linear(&self, su: f32, sv: f32) -> [f32; 3] {
        let w = self.image.width as usize;
        let h = self.image.height as usize;
        if w == 0 || h == 0 || self.image.rgba16f.len() < w * h * 4 {
            return [0.0; 3];
        }
        let px = ((su.clamp(0.0, 1.0) * (w - 1) as f32).round() as usize).min(w - 1);
        let py = ((sv.clamp(0.0, 1.0) * (h - 1) as f32).round() as usize).min(h - 1);
        let idx = (py * w + px) * 4;
        [
            self.image.rgba16f[idx].to_f32(),
            self.image.rgba16f[idx + 1].to_f32(),
            self.image.rgba16f[idx + 2].to_f32(),
        ]
    }

    fn sample_raw_patch_linear(&self, su: f32, sv: f32, radius: isize) -> [f32; 3] {
        let w = self.image.width as isize;
        let h = self.image.height as isize;
        if w <= 0 || h <= 0 || self.image.rgba16f.len() < (w * h * 4) as usize {
            return [0.0; 3];
        }
        let cx = ((su.clamp(0.0, 1.0) * (w - 1) as f32).round() as isize).clamp(0, w - 1);
        let cy = ((sv.clamp(0.0, 1.0) * (h - 1) as f32).round() as isize).clamp(0, h - 1);
        let mut acc = [0.0_f32; 3];
        let mut count = 0.0_f32;
        for dy in -radius..=radius {
            let y = (cy + dy).clamp(0, h - 1) as usize;
            for dx in -radius..=radius {
                let x = (cx + dx).clamp(0, w - 1) as usize;
                let idx = (y * w as usize + x) * 4;
                acc[0] += self.image.rgba16f[idx].to_f32();
                acc[1] += self.image.rgba16f[idx + 1].to_f32();
                acc[2] += self.image.rgba16f[idx + 2].to_f32();
                count += 1.0;
            }
        }
        let inv = if count > 0.0 { 1.0 / count } else { 0.0 };
        [acc[0] * inv, acc[1] * inv, acc[2] * inv]
    }

    // -------------------------------------------------------------------------
    // Interactive Crop Dragging (pixel-exact inverse of shaders/develop.wgsl)
    // -------------------------------------------------------------------------

    fn viewport_local_to_oriented_uv(
        &self,
        local_x: f32,
        local_y: f32,
        w_pt: f32,
        h_pt: f32,
    ) -> [f32; 2] {
        let scale = self.viewport_scale.max(1.0);
        let slot_uv = [local_x / w_pt.max(1.0), local_y / h_pt.max(1.0)];
        let slot_size = [(w_pt * scale).max(1.0), (h_pt * scale).max(1.0)];
        self.recipe.geometry.viewport_to_oriented_uv(
            slot_uv,
            slot_size,
            [self.image.width, self.image.height],
            &self.camera,
        )
    }

    pub fn begin_crop_drag(&mut self, local_x: f32, local_y: f32, w_pt: f32, h_pt: f32) {
        let uv = self.viewport_local_to_oriented_uv(local_x, local_y, w_pt, h_pt);
        let rect = self.recipe.geometry.sanitized_crop();
        let [x, y, w, h] = rect;
        let x2 = x + w;
        let y2 = y + h;
        let tol = 0.055 / self.camera.zoom.max(1.0);

        let near_l = (uv[0] - x).abs() <= tol && uv[1] >= y - tol && uv[1] <= y2 + tol;
        let near_r = (uv[0] - x2).abs() <= tol && uv[1] >= y - tol && uv[1] <= y2 + tol;
        let near_t = (uv[1] - y).abs() <= tol && uv[0] >= x - tol && uv[0] <= x2 + tol;
        let near_b = (uv[1] - y2).abs() <= tol && uv[0] >= x - tol && uv[0] <= x2 + tol;

        let handle = if near_l && near_t {
            Some(CropDragHandle::TopLeft)
        } else if near_r && near_t {
            Some(CropDragHandle::TopRight)
        } else if near_l && near_b {
            Some(CropDragHandle::BottomLeft)
        } else if near_r && near_b {
            Some(CropDragHandle::BottomRight)
        } else if near_l {
            Some(CropDragHandle::Left)
        } else if near_r {
            Some(CropDragHandle::Right)
        } else if near_t {
            Some(CropDragHandle::Top)
        } else if near_b {
            Some(CropDragHandle::Bottom)
        } else if uv[0] > x && uv[0] < x2 && uv[1] > y && uv[1] < y2 {
            Some(CropDragHandle::Move)
        } else {
            None
        };

        if let Some(hnd) = handle {
            if self.drag_undo_snapshot.is_none() {
                self.drag_undo_snapshot = Some(self.recipe.clone());
            }
            self.crop_drag = Some((hnd, uv, rect));
        }
    }

    pub fn update_crop_drag(&mut self, local_x: f32, local_y: f32, w_pt: f32, h_pt: f32) {
        let Some((handle, start_uv, [sx, sy, sw, sh])) = self.crop_drag else {
            return;
        };
        let uv = self.viewport_local_to_oriented_uv(local_x, local_y, w_pt, h_pt);
        let dx = uv[0] - start_uv[0];
        let dy = uv[1] - start_uv[1];
        const MIN_DIM: f32 = 0.08;

        let [ori_w, ori_h] = self
            .recipe
            .geometry
            .oriented_size(self.image.width, self.image.height);
        let ori_aspect = ori_w as f32 / ori_h.max(1) as f32;
        let norm_ratio = self
            .recipe
            .geometry
            .aspect_preset
            .ratio(ori_aspect)
            .map(|r| r / ori_aspect);

        let mut x1 = sx;
        let mut y1 = sy;
        let mut x2 = sx + sw;
        let mut y2 = sy + sh;

        match handle {
            CropDragHandle::Move => {
                let nx = (sx + dx).clamp(0.0, 1.0 - sw);
                let ny = (sy + dy).clamp(0.0, 1.0 - sh);
                self.recipe.geometry.crop = [nx, ny, sw, sh];
                self.recipe.preset_name = None;
                return;
            }
            CropDragHandle::TopLeft => {
                x1 = (sx + dx).clamp(0.0, x2 - MIN_DIM);
                y1 = (sy + dy).clamp(0.0, y2 - MIN_DIM);
            }
            CropDragHandle::TopRight => {
                x2 = (sx + sw + dx).clamp(x1 + MIN_DIM, 1.0);
                y1 = (sy + dy).clamp(0.0, y2 - MIN_DIM);
            }
            CropDragHandle::BottomLeft => {
                x1 = (sx + dx).clamp(0.0, x2 - MIN_DIM);
                y2 = (sy + sh + dy).clamp(y1 + MIN_DIM, 1.0);
            }
            CropDragHandle::BottomRight => {
                x2 = (sx + sw + dx).clamp(x1 + MIN_DIM, 1.0);
                y2 = (sy + sh + dy).clamp(y1 + MIN_DIM, 1.0);
            }
            CropDragHandle::Left => x1 = (sx + dx).clamp(0.0, x2 - MIN_DIM),
            CropDragHandle::Right => x2 = (sx + sw + dx).clamp(x1 + MIN_DIM, 1.0),
            CropDragHandle::Top => y1 = (sy + dy).clamp(0.0, y2 - MIN_DIM),
            CropDragHandle::Bottom => y2 = (sy + sh + dy).clamp(y1 + MIN_DIM, 1.0),
        }

        // Enforce locked aspect ratio cleanly when preset != Free
        if let Some(nr) = norm_ratio
            && nr > 1e-4
        {
            let mut nw = (x2 - x1).max(MIN_DIM);
            let mut nh = (y2 - y1).max(MIN_DIM);
            let drive_by_h = matches!(handle, CropDragHandle::Top | CropDragHandle::Bottom);
            if drive_by_h {
                nw = (nh * nr).max(MIN_DIM);
            } else {
                nh = (nw / nr).max(MIN_DIM);
            }
            // Anchor opposite edge/corner
            match handle {
                CropDragHandle::TopLeft => {
                    let max_w = x2.min(y2 * nr);
                    nw = nw.min(max_w);
                    nh = nw / nr;
                    x1 = x2 - nw;
                    y1 = y2 - nh;
                }
                CropDragHandle::TopRight => {
                    let max_w = (1.0 - x1).min(y2 * nr);
                    nw = nw.min(max_w);
                    nh = nw / nr;
                    x2 = x1 + nw;
                    y1 = y2 - nh;
                }
                CropDragHandle::BottomLeft => {
                    let max_w = x2.min((1.0 - y1) * nr);
                    nw = nw.min(max_w);
                    nh = nw / nr;
                    x1 = x2 - nw;
                    y2 = y1 + nh;
                }
                CropDragHandle::BottomRight => {
                    let max_w = (1.0 - x1).min((1.0 - y1) * nr);
                    nw = nw.min(max_w);
                    nh = nw / nr;
                    x2 = x1 + nw;
                    y2 = y1 + nh;
                }
                CropDragHandle::Left => {
                    let cy = sy + sh * 0.5;
                    let max_w = x2.min(1.0 * nr);
                    nw = nw.min(max_w);
                    nh = (nw / nr).min(1.0);
                    nw = nh * nr;
                    x1 = x2 - nw;
                    y1 = (cy - nh * 0.5).clamp(0.0, 1.0 - nh);
                    y2 = y1 + nh;
                }
                CropDragHandle::Right => {
                    let cy = sy + sh * 0.5;
                    let max_w = (1.0 - x1).min(1.0 * nr);
                    nw = nw.min(max_w);
                    nh = (nw / nr).min(1.0);
                    nw = nh * nr;
                    x2 = x1 + nw;
                    y1 = (cy - nh * 0.5).clamp(0.0, 1.0 - nh);
                    y2 = y1 + nh;
                }
                CropDragHandle::Top => {
                    let cx = sx + sw * 0.5;
                    let max_h = y2.min(1.0 / nr);
                    nh = nh.min(max_h);
                    nw = (nh * nr).min(1.0);
                    nh = nw / nr;
                    y1 = y2 - nh;
                    x1 = (cx - nw * 0.5).clamp(0.0, 1.0 - nw);
                    x2 = x1 + nw;
                }
                CropDragHandle::Bottom => {
                    let cx = sx + sw * 0.5;
                    let max_h = (1.0 - y1).min(1.0 / nr);
                    nh = nh.min(max_h);
                    nw = (nh * nr).min(1.0);
                    nh = nw / nr;
                    y2 = y1 + nh;
                    x1 = (cx - nw * 0.5).clamp(0.0, 1.0 - nw);
                    x2 = x1 + nw;
                }
                CropDragHandle::Move => {}
            }
        }

        self.recipe.geometry.crop = [
            x1.clamp(0.0, 0.99),
            y1.clamp(0.0, 0.99),
            (x2 - x1).clamp(0.01, 1.0 - x1.clamp(0.0, 0.99)),
            (y2 - y1).clamp(0.01, 1.0 - y1.clamp(0.0, 0.99)),
        ];
        self.recipe.preset_name = None;
    }

    pub fn end_crop_drag_tx(&mut self, tx: &mut Tx) {
        self.crop_drag = None;
        self.catalog.update_active_recipe(self.recipe.clone());
        if let Some(prev) = self.drag_undo_snapshot.take()
            && prev != self.recipe
        {
            self.history.push(prev);
        }
        self.sync_window_state(tx);
    }

    pub fn set_crop_aspect_tx(&mut self, preset: CropAspect, tx: &mut Tx) {
        let mut next = self.recipe.clone();
        next.geometry
            .apply_aspect_preset(preset, self.image.width, self.image.height);
        next.preset_name = None;
        if next != self.recipe {
            self.history.push(self.recipe.clone());
            self.recipe = next;
            self.catalog.update_active_recipe(self.recipe.clone());
            self.status_message = Some(format!("Crop aspect: {}", preset.label()));
            self.sync_window_state(tx);
        }
    }

    pub fn apply_preset(&mut self, preset_id: &str) {
        let Some(preset) = self.presets.iter().find(|p| p.id == preset_id).cloned() else {
            return;
        };
        let strength = self.style_strength.clamp(0.0, 1.0);
        let mut next = if self.stack_styles {
            let mut stacked = self.recipe.clone();
            let patch = recipe_to_sparse_patch(&preset.recipe);
            let _ = stack_style_onto_recipe(&mut stacked, &patch, strength);
            stacked.preset_name = None;
            stacked
        } else if (strength - 1.0).abs() > 1e-3 {
            let mut blended = rune_editor_core::blend_recipes(
                &DevelopRecipe::default(),
                &preset.recipe,
                strength,
            );
            blended.geometry = self.recipe.geometry;
            blended.preset_name = None;
            blended
        } else {
            let mut replaced = preset.recipe;
            replaced.geometry = self.recipe.geometry;
            replaced
        };
        next.geometry = self.recipe.geometry;
        let label = if self.stack_styles {
            format!("Stacked style: {} ({:.0}%)", preset.name, strength * 100.0)
        } else {
            format!("Applied style: {}", preset.name)
        };
        self.commit_recipe_change(next, label);
    }

    pub fn apply_preset_tx(&mut self, preset_id: &str, tx: &mut Tx) {
        self.apply_preset(preset_id);
        self.sync_window_state(tx);
    }

    // -------------------------------------------------------------------------
    // Phase 3: Virtual Variants, Compare Variant, Selective Clipboard, User Styles,
    // 120Hz Loupe, Pinned Color Readouts & Multi-Recipe Export
    // -------------------------------------------------------------------------

    pub fn active_variant_count(&self) -> usize {
        self.catalog
            .active_meta()
            .map_or(1, PhotoMeta::variant_count)
    }

    pub fn active_variant_index(&self) -> usize {
        self.catalog
            .active_meta()
            .map_or(0, |m| m.active_variant_index)
    }

    pub fn new_variant(&mut self, label: Option<String>) -> usize {
        self.catalog.update_active_recipe(self.recipe.clone());
        let idx = if let Some(entry) = self.catalog.active_entry_mut() {
            let i = entry.meta.new_variant(label);
            entry.color_tag = entry.meta.color_tag;
            self.recipe = entry.meta.recipe.clone();
            self.saved_recipe = self.recipe.clone();
            i
        } else {
            0
        };
        self.clear_transient_interactions();
        self.history.clear();
        self.sync_profile_state_after_recipe_change();
        self.recompute_histogram();
        self.status_message = Some(format!("Created Variant V{} (Neutral)", idx + 1));
        idx
    }

    pub fn new_variant_tx(&mut self, label: Option<String>, tx: &mut Tx) {
        self.new_variant(label);
        if !self.demo_mode
            && let Some(idx) = self.catalog.active_index
        {
            let _ = self.catalog.save_entry_sidecar_mut(idx);
        }
        self.sync_window_state(tx);
    }

    pub fn clone_variant(&mut self, label: Option<String>) -> usize {
        self.catalog.update_active_recipe(self.recipe.clone());
        let idx = if let Some(entry) = self.catalog.active_entry_mut() {
            let i = entry.meta.clone_variant(label);
            entry.color_tag = entry.meta.color_tag;
            self.recipe = entry.meta.recipe.clone();
            self.saved_recipe = self.recipe.clone();
            i
        } else {
            0
        };
        self.clear_transient_interactions();
        self.history.clear();
        self.recompute_histogram();
        self.status_message = Some(format!("Cloned Variant V{}", idx + 1));
        idx
    }

    pub fn clone_variant_tx(&mut self, label: Option<String>, tx: &mut Tx) {
        self.clone_variant(label);
        if !self.demo_mode
            && let Some(idx) = self.catalog.active_index
        {
            let _ = self.catalog.save_entry_sidecar_mut(idx);
        }
        self.sync_window_state(tx);
    }

    pub fn select_variant(&mut self, index: usize) -> bool {
        self.catalog.update_active_recipe(self.recipe.clone());
        let switched = if let Some(entry) = self.catalog.active_entry_mut() {
            if entry.meta.select_variant(index) {
                entry.color_tag = entry.meta.color_tag;
                self.recipe = entry.meta.recipe.clone();
                self.saved_recipe = self.recipe.clone();
                true
            } else {
                false
            }
        } else {
            false
        };
        if switched {
            self.clear_transient_interactions();
            self.history.clear();
            self.sync_profile_state_after_recipe_change();
            self.recompute_histogram();
            self.status_message = Some(format!("Selected Variant V{}", index + 1));
        }
        switched
    }

    pub fn select_variant_tx(&mut self, index: usize, tx: &mut Tx) {
        if self.select_variant(index) {
            if !self.demo_mode
                && let Some(idx) = self.catalog.active_index
            {
                let _ = self.catalog.save_entry_sidecar_mut(idx);
            }
            self.sync_window_state(tx);
        }
    }

    pub fn next_variant_tx(&mut self, tx: &mut Tx) {
        let count = self.active_variant_count();
        if count > 1 {
            let next = (self.active_variant_index() + 1) % count;
            self.select_variant_tx(next, tx);
        }
    }

    pub fn delete_active_variant(&mut self) -> bool {
        let cur = self.active_variant_index();
        let deleted = if let Some(entry) = self.catalog.active_entry_mut() {
            if entry.meta.delete_variant(cur) {
                entry.color_tag = entry.meta.color_tag;
                self.recipe = entry.meta.recipe.clone();
                self.saved_recipe = self.recipe.clone();
                true
            } else {
                false
            }
        } else {
            false
        };
        if deleted {
            self.clear_transient_interactions();
            self.history.clear();
            self.sync_profile_state_after_recipe_change();
            self.recompute_histogram();
            self.status_message = Some(format!(
                "Deleted variant • Now on V{}",
                self.active_variant_index() + 1
            ));
        }
        deleted
    }

    pub fn delete_active_variant_tx(&mut self, tx: &mut Tx) {
        if self.delete_active_variant() {
            if !self.demo_mode
                && let Some(idx) = self.catalog.active_index
            {
                let _ = self.catalog.save_entry_sidecar_mut(idx);
            }
            self.sync_window_state(tx);
        }
    }

    /// Pins the current frame & active variant as a Capture One orange-bordered Compare Reference.
    pub fn pin_compare_variant(&mut self) {
        let pixels = render_graded_demo_pixels(&self.image, &self.recipe, 960);
        let variant_tag = format!("V{}", self.active_variant_index() + 1);
        let label = self.image.display_name.clone();
        self.compare_reference = Some(CompareReference {
            label: label.clone(),
            variant_tag: variant_tag.clone(),
            pixels,
        });
        self.camera.split_compare = false;
        self.show_grid_browser = false;
        self.status_message = Some(format!(
            "Compare Variant pinned: {label} ({variant_tag}) — switch variant or photo to compare"
        ));
    }

    pub fn toggle_compare_variant_tx(&mut self, tx: &mut Tx) {
        if self.compare_reference.is_some() {
            self.compare_reference = None;
            self.status_message = Some("Cleared Compare Variant reference".to_owned());
        } else {
            self.pin_compare_variant();
        }
        self.sync_window_state(tx);
    }

    // -------------------------------------------------------------------------
    // Phase 4: Adjustment Layers, Composable Masks, AI Subject/Bg & Heal/Clone
    // -------------------------------------------------------------------------

    /// Keeps `active_layer_index` within bounds and mirrors it into `camera.active_layer_index`.
    pub fn sync_active_layer_camera(&mut self) {
        let len = self.recipe.layers.len().min(MAX_ADJUSTMENT_LAYERS);
        if len == 0 {
            self.active_layer_index = None;
            self.camera.active_layer_index = -1;
            self.camera.mask_overlay_mode = 0;
        } else if let Some(idx) = self.active_layer_index {
            let clamped = idx.min(len - 1);
            self.active_layer_index = Some(clamped);
            self.camera.active_layer_index = clamped as i32;
        } else {
            self.camera.active_layer_index = -1;
            self.camera.mask_overlay_mode = 0;
        }
    }

    pub fn select_layer(&mut self, index: Option<usize>) {
        let len = self.recipe.layers.len().min(MAX_ADJUSTMENT_LAYERS);
        let next_idx = index.filter(|&i| i < len);
        if next_idx != self.ai_select_target_layer {
            self.ai_select_points.clear();
            self.ai_select_bbox = None;
            self.ai_select_bbox_preview = None;
            self.ai_select_invert = false;
            self.ai_select_target_layer = next_idx;
        }
        self.active_layer_index = next_idx;
        self.sync_active_layer_camera();
        self.status_message = Some(match self.active_layer_index {
            Some(i) => format!("Selected layer: {}", self.recipe.layers[i].name),
            None => "Selected Background (Base Image)".to_owned(),
        });
    }

    pub fn select_layer_tx(&mut self, index: Option<usize>, tx: &mut Tx) {
        self.select_layer(index);
        self.sync_window_state(tx);
    }

    pub fn set_canvas_tool(&mut self, tool: CanvasTool) {
        self.active_tool = tool;
        self.layer_drag = None;
        self.ai_select_bbox_preview = None;
        if tool.is_retouch_or_mask() {
            self.camera.crop_mode = false;
            self.crop_drag = None;
            self.wb_eyedropper_active = false;
            self.color_slice_picker_active = false;
            self.skin_picker_active = false;
            self.readout_picker_active = false;
            self.camera.view_color_range = -1;
            self.show_grid_browser = false;
            self.active_tab = InspectorTab::Layers;
            if matches!(tool, CanvasTool::Heal | CanvasTool::Clone) {
                self.heal_open = true;
            }
            if tool == CanvasTool::AiSelect {
                self.layers_open = true;
                self.ai_select_target_layer = self.active_layer_index;
            }
        }
        self.status_message = Some(match tool {
            CanvasTool::Hand => "Hand cursor active".to_owned(),
            CanvasTool::AiSelect => {
                "AI Select: Click to include · ⌥Click to exclude · ⇧Drag for box · Esc done"
                    .to_owned()
            }
            _ => format!("Tool: {} — drag on photo • Esc for Hand", tool.label()),
        });
    }

    pub fn set_canvas_tool_tx(&mut self, tool: CanvasTool, tx: &mut Tx) {
        self.set_canvas_tool(tool);
        self.sync_window_state(tx);
    }

    pub fn set_mask_overlay_mode(&mut self, mode: i32) {
        let clamped = mode.clamp(0, 2);
        if clamped != 0 && self.active_layer_index.is_none() && !self.recipe.layers.is_empty() {
            self.active_layer_index = Some(self.recipe.layers.len() - 1);
        }
        self.sync_active_layer_camera();
        if self.active_layer_index.is_none() && clamped != 0 {
            self.camera.mask_overlay_mode = 0;
            self.status_message =
                Some("Add or select an adjustment layer to view its mask".to_owned());
            return;
        }
        self.camera.mask_overlay_mode = clamped;
        self.status_message = Some(match clamped {
            1 => "Mask Overlay: Red Rubylith (M)".to_owned(),
            2 => "Mask Overlay: Grayscale (⌥M)".to_owned(),
            _ => "Mask Overlay: Off".to_owned(),
        });
    }

    pub fn set_mask_overlay_mode_tx(&mut self, mode: i32, tx: &mut Tx) {
        self.set_mask_overlay_mode(mode);
        self.sync_window_state(tx);
    }

    pub fn toggle_red_mask_overlay(&mut self) {
        let next = if self.camera.mask_overlay_mode == 1 {
            0
        } else {
            1
        };
        self.set_mask_overlay_mode(next);
    }

    pub fn toggle_red_mask_overlay_tx(&mut self, tx: &mut Tx) {
        self.toggle_red_mask_overlay();
        self.sync_window_state(tx);
    }

    pub fn toggle_gray_mask_overlay(&mut self) {
        let next = if self.camera.mask_overlay_mode == 2 {
            0
        } else {
            2
        };
        self.set_mask_overlay_mode(next);
    }

    pub fn toggle_gray_mask_overlay_tx(&mut self, tx: &mut Tx) {
        self.toggle_gray_mask_overlay();
        self.sync_window_state(tx);
    }

    fn next_layer_id(&self) -> String {
        let mut n = self.recipe.layers.len() + 1;
        while self
            .recipe
            .layers
            .iter()
            .any(|l| l.id == format!("layer-{n}"))
        {
            n += 1;
        }
        format!("layer-{n}")
    }

    fn next_heal_spot_id(&self) -> String {
        let mut n = self.recipe.heal_spots.len() + 1;
        while self
            .recipe
            .heal_spots
            .iter()
            .any(|h| h.id == format!("heal-{n}"))
        {
            n += 1;
        }
        format!("heal-{n}")
    }

    pub fn add_layer_with_shape(
        &mut self,
        name: String,
        shape: Option<MaskShape>,
    ) -> Option<usize> {
        if self.recipe.layers.len() >= MAX_ADJUSTMENT_LAYERS {
            self.status_message = Some(format!(
                "Maximum {MAX_ADJUSTMENT_LAYERS} adjustment layers reached"
            ));
            return None;
        }
        let id = self.next_layer_id();
        let layer = match shape {
            Some(s) => AdjustmentLayer::new_with_mask(id, name.clone(), s),
            None => AdjustmentLayer::new(id, name.clone()),
        };
        let mut next = self.recipe.clone();
        next.layers.push(layer);
        next.preset_name = None;
        let idx = next.layers.len() - 1;
        self.active_layer_index = Some(idx);
        self.commit_recipe_change(next, format!("Added layer: {name}"));
        Some(idx)
    }

    pub fn add_empty_layer_tx(&mut self, tx: &mut Tx) {
        let n = self.recipe.layers.len() + 1;
        self.add_layer_with_shape(format!("Adjustment {n}"), None);
        self.sync_window_state(tx);
    }

    pub fn add_filled_layer_tx(&mut self, tx: &mut Tx) {
        let n = self.recipe.layers.len() + 1;
        self.add_layer_with_shape(format!("Filled Layer {n}"), Some(MaskShape::Filled));
        self.sync_window_state(tx);
    }

    pub fn add_linear_layer_tx(&mut self, tx: &mut Tx) {
        let n = self.recipe.layers.len() + 1;
        let shape = MaskShape::LinearGradient {
            start: [0.5, 0.16],
            end: [0.5, 0.62],
            feather: 0.75,
        };
        if self
            .add_layer_with_shape(format!("Linear Gradient {n}"), Some(shape))
            .is_some()
        {
            self.set_canvas_tool(CanvasTool::LinearGradient);
        }
        self.sync_window_state(tx);
    }

    pub fn add_radial_layer_tx(&mut self, tx: &mut Tx) {
        let n = self.recipe.layers.len() + 1;
        let shape = MaskShape::RadialGradient {
            center: [0.5, 0.46],
            radius: [0.28, 0.34],
            angle_deg: 0.0,
            feather: 0.45,
        };
        if self
            .add_layer_with_shape(format!("Radial Gradient {n}"), Some(shape))
            .is_some()
        {
            self.set_canvas_tool(CanvasTool::RadialGradient);
        }
        self.sync_window_state(tx);
    }

    pub fn add_luma_layer_tx(&mut self, tx: &mut Tx) {
        let n = self.recipe.layers.len() + 1;
        let shape = MaskShape::LumaRange {
            low: 0.45,
            high: 1.0,
            low_falloff: 0.15,
            high_falloff: 0.05,
        };
        self.add_layer_with_shape(format!("Luma Range {n}"), Some(shape));
        self.sync_window_state(tx);
    }

    fn apply_ai_subject_result(
        &mut self,
        background: bool,
        mask: MaskShape,
        status: AiMaskStatus,
    ) -> Option<usize> {
        let name = if background {
            "AI Background".to_owned()
        } else {
            "AI Subject".to_owned()
        };
        let idx = self.add_layer_with_shape(name.clone(), Some(mask))?;
        let mut msg = format!("{name} · {}", status.compact_badge());
        if let Some(note) = &status.fallback_note {
            msg.push_str(&format!(" ({note})"));
        }
        self.last_ai_mask_status = Some(status);
        self.status_message = Some(msg);
        Some(idx)
    }

    pub fn add_ai_subject_layer(&mut self, background: bool) -> Option<usize> {
        let prompt = if background {
            SmartMaskPrompt::AutoBackground
        } else {
            SmartMaskPrompt::AutoSubject
        };

        let (mask, status) = match generate_smart_mask(&self.image, &prompt, None) {
            Ok(res) => {
                let status = AiMaskStatus::from_result(&res);
                (res.mask, status)
            }
            Err(err) => {
                let mask = generate_heuristic_mask(&self.image, background);
                let status = AiMaskStatus {
                    engine: SegmentationEngine::Heuristic,
                    backend_label: "Heuristic Fallback".to_owned(),
                    elapsed_ms: 0,
                    iou_score: None,
                    instance_count: None,
                    fallback_note: Some(err.to_string()),
                };
                (mask, status)
            }
        };

        self.apply_ai_subject_result(background, mask, status)
    }

    pub fn add_ai_subject_layer_tx(&mut self, background: bool, tx: &mut Tx) {
        if let Some(handle) = self.app_handle.clone() {
            let prompt = if background {
                SmartMaskPrompt::AutoBackground
            } else {
                SmartMaskPrompt::AutoSubject
            };
            let label = if background {
                "Segmenting background…".to_owned()
            } else {
                "Segmenting subject…".to_owned()
            };
            let req_id = self.ai_seg_request_seq.fetch_add(1, Ordering::AcqRel) + 1;
            let image = self.image.clone();
            let expected_src = self.image.source_path.clone();
            self.ai_segmenting_label = Some(label.clone());
            self.status_message = Some(label);
            self.sync_window_state(tx);

            std::thread::spawn(move || {
                let (mask, status) = match generate_smart_mask(&image, &prompt, None) {
                    Ok(res) => {
                        let status = AiMaskStatus::from_result(&res);
                        (res.mask, status)
                    }
                    Err(err) => {
                        let mask = generate_heuristic_mask(&image, background);
                        let status = AiMaskStatus {
                            engine: SegmentationEngine::Heuristic,
                            backend_label: "Heuristic Fallback".to_owned(),
                            elapsed_ms: 0,
                            iou_score: None,
                            instance_count: None,
                            fallback_note: Some(err.to_string()),
                        };
                        (mask, status)
                    }
                };
                handle.update_if(move |s, tx| {
                    if s.ai_seg_request_seq.load(Ordering::Acquire) != req_id {
                        return false;
                    }
                    s.ai_segmenting_label = None;
                    if s.image.source_path == expected_src
                        && s.apply_ai_subject_result(background, mask, status)
                            .is_some()
                    {
                        s.set_mask_overlay_mode(1);
                    }
                    s.sync_window_state(tx);
                    true
                });
            });
            return;
        }
        if self.add_ai_subject_layer(background).is_some() {
            self.set_mask_overlay_mode(1);
        }
        self.sync_window_state(tx);
    }

    fn apply_ai_select_result(
        &mut self,
        result: Result<SmartMaskResult, rune_editor_decode::VisionError>,
    ) -> bool {
        match result {
            Ok(res) => {
                let status = AiMaskStatus::from_result(&res);
                let mut next = self.recipe.clone();
                let target_idx = self
                    .ai_select_target_layer
                    .or(self.active_layer_index)
                    .filter(|&i| i < next.layers.len());

                let layer_idx = if let Some(idx) = target_idx {
                    // Inversion is already baked into the prompt's RasterMask bytes; avoid double-invert.
                    next.layers[idx].inverted = false;
                    if let Some(mi) = next.layers[idx]
                        .masks
                        .iter()
                        .rposition(|m| matches!(m.shape, MaskShape::RasterMask { .. }))
                    {
                        next.layers[idx].masks[mi].shape = res.mask;
                    } else if next.layers[idx].masks.len() < MAX_MASKS_PER_LAYER {
                        next.layers[idx].masks.push(LayerMaskItem::new(res.mask));
                    } else if let Some(last) = next.layers[idx].masks.last_mut() {
                        last.shape = res.mask;
                    }
                    idx
                } else {
                    if next.layers.len() >= MAX_ADJUSTMENT_LAYERS {
                        self.status_message = Some(format!(
                            "Maximum {MAX_ADJUSTMENT_LAYERS} adjustment layers reached"
                        ));
                        return false;
                    }
                    let id = self.next_layer_id();
                    next.layers.push(AdjustmentLayer::new_with_mask(
                        id,
                        "AI Selection".to_owned(),
                        res.mask,
                    ));
                    next.layers.len() - 1
                };

                next.preset_name = None;
                self.active_layer_index = Some(layer_idx);
                self.ai_select_target_layer = Some(layer_idx);
                let mut msg = format!("AI Select · {}", status.compact_badge());
                if let Some(note) = &status.fallback_note {
                    msg.push_str(&format!(" ({note})"));
                }
                self.last_ai_mask_status = Some(status);
                self.commit_recipe_change(next, msg);
                if self.camera.mask_overlay_mode == 0 {
                    self.camera.mask_overlay_mode = 1;
                }
                self.sync_active_layer_camera();
                true
            }
            Err(err) => {
                self.status_message = Some(format!("AI Select failed: {err}"));
                false
            }
        }
    }

    /// Runs interactive `AI Select` segmentation for current `ai_select_points` + `ai_select_bbox`,
    /// offloading inference to a worker thread when a live `AppHandle` is attached so the 120Hz UI
    /// thread never stalls during ONNX model loading or encoding.
    pub fn run_ai_select_segmentation(&mut self) -> bool {
        if self.ai_select_points.is_empty() && self.ai_select_bbox.is_none() {
            return false;
        }
        let prompt = SmartMaskPrompt::Interactive {
            points: self.ai_select_points.clone(),
            bbox: self.ai_select_bbox,
            invert: self.ai_select_invert,
        };
        if let Some(handle) = self.app_handle.clone() {
            let req_id = self.ai_seg_request_seq.fetch_add(1, Ordering::AcqRel) + 1;
            let image = self.image.clone();
            let expected_src = self.image.source_path.clone();
            self.ai_segmenting_label = Some("Segmenting selection…".to_owned());
            self.status_message = self.ai_segmenting_label.clone();

            std::thread::spawn(move || {
                let result = generate_smart_mask(&image, &prompt, None);
                handle.update_if(move |s, tx| {
                    if s.ai_seg_request_seq.load(Ordering::Acquire) != req_id {
                        return false;
                    }
                    s.ai_segmenting_label = None;
                    if s.image.source_path == expected_src {
                        s.apply_ai_select_result(result);
                    }
                    s.sync_window_state(tx);
                    true
                });
            });
            return true;
        }
        let result = generate_smart_mask(&self.image, &prompt, None);
        self.apply_ai_select_result(result)
    }

    /// Appends a normalized `[0, 1]` prompt point (`+ Include` or `− Exclude`) and segments immediately.
    pub fn ai_select_add_point(&mut self, u: f32, v: f32, positive: bool) -> bool {
        if self.ai_select_points.len() >= MAX_PROMPT_POINTS {
            self.status_message = Some(format!(
                "Maximum {MAX_PROMPT_POINTS} AI Select prompt points reached"
            ));
            return false;
        }
        self.ai_select_points
            .push(PromptPoint::new(u, v, positive).sanitized());
        self.run_ai_select_segmentation()
    }

    /// Sets the interactive bounding box (`PromptBox`) and runs segmentation immediately.
    pub fn ai_select_set_bbox(&mut self, bbox: PromptBox) -> bool {
        let clean = bbox.sanitized();
        if !clean.is_valid() {
            return false;
        }
        self.ai_select_bbox = Some(clean);
        self.ai_select_bbox_preview = None;
        self.run_ai_select_segmentation()
    }

    /// Toggles `AI Select` mask inversion and re-segments (or flips active layer invert if no prompts).
    pub fn ai_select_toggle_invert(&mut self) -> bool {
        self.ai_select_invert = !self.ai_select_invert;
        if !self.ai_select_points.is_empty() || self.ai_select_bbox.is_some() {
            self.run_ai_select_segmentation()
        } else {
            self.toggle_active_layer_invert()
        }
    }

    pub fn ai_select_toggle_invert_tx(&mut self, tx: &mut Tx) {
        if self.ai_select_toggle_invert() {
            self.sync_window_state(tx);
        }
    }

    /// Clears interactive prompt points & bounding box without discarding the active layer mask.
    pub fn ai_select_clear_points(&mut self) {
        self.ai_select_points.clear();
        self.ai_select_bbox = None;
        self.ai_select_bbox_preview = None;
        self.ai_select_invert = false;
        self.status_message = Some("Cleared AI Select prompts".to_owned());
    }

    pub fn ai_select_clear_points_tx(&mut self, tx: &mut Tx) {
        self.ai_select_clear_points();
        self.sync_window_state(tx);
    }

    /// Finishes the interactive `AI Select` session, keeping the mask and returning to Hand cursor.
    pub fn ai_select_done(&mut self) {
        self.ai_select_points.clear();
        self.ai_select_bbox = None;
        self.ai_select_bbox_preview = None;
        self.ai_select_invert = false;
        self.ai_select_target_layer = None;
        self.set_canvas_tool(CanvasTool::Hand);
    }

    pub fn ai_select_done_tx(&mut self, tx: &mut Tx) {
        self.ai_select_done();
        self.sync_window_state(tx);
    }

    /// Clears all interactive AI Select prompt state & telemetry when switching photos.
    pub fn reset_ai_select_for_photo_switch(&mut self) {
        self.ai_select_points.clear();
        self.ai_select_bbox = None;
        self.ai_select_bbox_preview = None;
        self.ai_select_invert = false;
        self.ai_select_target_layer = None;
        self.last_ai_mask_status = None;
    }

    pub fn duplicate_layer_tx(&mut self, idx: usize, tx: &mut Tx) {
        if idx >= self.recipe.layers.len() || self.recipe.layers.len() >= MAX_ADJUSTMENT_LAYERS {
            return;
        }
        let mut cloned = self.recipe.layers[idx].clone();
        cloned.id = self.next_layer_id();
        cloned.name = format!("{} Copy", cloned.name);
        let mut next = self.recipe.clone();
        let insert_at = idx + 1;
        next.layers.insert(insert_at, cloned);
        next.preset_name = None;
        if let Some(t) = self.ai_select_target_layer
            && insert_at <= t
        {
            self.ai_select_target_layer = Some(t + 1);
        }
        self.active_layer_index = Some(insert_at);
        self.commit_recipe_change(next, "Duplicated adjustment layer");
        self.sync_window_state(tx);
    }

    pub fn remove_layer(&mut self, idx: usize) -> bool {
        if idx >= self.recipe.layers.len() {
            return false;
        }
        let mut next = self.recipe.clone();
        let removed = next.layers.remove(idx);
        next.preset_name = None;
        if next.layers.is_empty() {
            self.active_layer_index = None;
        } else if let Some(cur) = self.active_layer_index {
            if cur >= next.layers.len() {
                self.active_layer_index = Some(next.layers.len() - 1);
            } else if idx < cur {
                self.active_layer_index = Some(cur - 1);
            }
        }
        if let Some(t) = self.ai_select_target_layer {
            if t == idx {
                self.ai_select_target_layer = None;
                self.ai_select_points.clear();
                self.ai_select_bbox = None;
                self.ai_select_bbox_preview = None;
                self.ai_select_invert = false;
            } else if idx < t {
                self.ai_select_target_layer = Some(t - 1);
            }
        }
        self.commit_recipe_change(next, format!("Deleted layer: {}", removed.name));
        true
    }

    pub fn remove_layer_tx(&mut self, idx: usize, tx: &mut Tx) {
        if self.remove_layer(idx) {
            self.sync_window_state(tx);
        }
    }

    pub fn move_layer(&mut self, idx: usize, delta: isize) -> bool {
        let len = self.recipe.layers.len();
        if idx >= len {
            return false;
        }
        let target = idx as isize + delta;
        if target < 0 || target >= len as isize {
            return false;
        }
        let target = target as usize;
        let mut next = self.recipe.clone();
        next.layers.swap(idx, target);
        next.preset_name = None;
        if self.active_layer_index == Some(idx) {
            self.active_layer_index = Some(target);
        } else if self.active_layer_index == Some(target) {
            self.active_layer_index = Some(idx);
        }
        if self.ai_select_target_layer == Some(idx) {
            self.ai_select_target_layer = Some(target);
        } else if self.ai_select_target_layer == Some(target) {
            self.ai_select_target_layer = Some(idx);
        }
        self.commit_recipe_change(next, "Reordered adjustment layer");
        true
    }

    pub fn move_layer_tx(&mut self, idx: usize, delta: isize, tx: &mut Tx) {
        if self.move_layer(idx, delta) {
            self.sync_window_state(tx);
        }
    }

    pub fn toggle_layer_enabled_tx(&mut self, idx: usize, tx: &mut Tx) {
        if idx >= self.recipe.layers.len() {
            return;
        }
        let mut next = self.recipe.clone();
        next.layers[idx].enabled = !next.layers[idx].enabled;
        let on = next.layers[idx].enabled;
        let name = next.layers[idx].name.clone();
        self.commit_recipe_change(
            next,
            format!("{}: {}", name, if on { "Visible" } else { "Hidden" }),
        );
        self.sync_window_state(tx);
    }

    pub fn toggle_active_layer_invert(&mut self) -> bool {
        let Some(idx) = self.active_layer_index else {
            return false;
        };
        if idx >= self.recipe.layers.len() {
            return false;
        }
        let mut next = self.recipe.clone();
        next.layers[idx].inverted = !next.layers[idx].inverted;
        let inv = next.layers[idx].inverted;
        self.commit_recipe_change(
            next,
            if inv {
                "Inverted active layer mask"
            } else {
                "Restored active layer mask polarity"
            },
        );
        true
    }

    pub fn toggle_active_layer_invert_tx(&mut self, tx: &mut Tx) {
        if self.toggle_active_layer_invert() {
            self.sync_window_state(tx);
        }
    }

    pub fn clear_active_layer_masks_tx(&mut self, tx: &mut Tx) {
        let Some(idx) = self.active_layer_index else {
            return;
        };
        if idx >= self.recipe.layers.len() {
            return;
        }
        let mut next = self.recipe.clone();
        next.layers[idx].masks.clear();
        next.layers[idx].inverted = false;
        self.commit_recipe_change(next, "Cleared active layer masks");
        self.sync_window_state(tx);
    }

    pub fn reset_active_layer_adjustments_tx(&mut self, tx: &mut Tx) {
        let Some(idx) = self.active_layer_index else {
            return;
        };
        if idx >= self.recipe.layers.len() {
            return;
        }
        let mut next = self.recipe.clone();
        next.layers[idx].adjustments = LocalAdjustments::default();
        self.commit_recipe_change(next, "Reset local adjustments on active layer");
        self.sync_window_state(tx);
    }

    pub fn add_mask_to_active_layer(&mut self, shape: MaskShape) -> bool {
        let Some(idx) = self.active_layer_index else {
            return false;
        };
        if idx >= self.recipe.layers.len()
            || self.recipe.layers[idx].masks.len() >= MAX_MASKS_PER_LAYER
        {
            return false;
        }
        let label = shape.kind_label();
        let mut next = self.recipe.clone();
        next.layers[idx].masks.push(LayerMaskItem::new(shape));
        self.commit_recipe_change(next, format!("Added {label} mask to layer"));
        true
    }

    pub fn add_mask_to_active_layer_tx(&mut self, shape: MaskShape, tx: &mut Tx) {
        if self.add_mask_to_active_layer(shape) {
            self.sync_window_state(tx);
        }
    }

    pub fn set_layer_mask_op(
        &mut self,
        layer_idx: usize,
        mask_idx: usize,
        op: MaskCombineOp,
    ) -> bool {
        if layer_idx >= self.recipe.layers.len()
            || mask_idx >= self.recipe.layers[layer_idx].masks.len()
        {
            return false;
        }
        let mut next = self.recipe.clone();
        next.layers[layer_idx].masks[mask_idx].op = op;
        self.commit_recipe_change(next, format!("Mask combine mode: {}", op.label()));
        true
    }

    pub fn set_layer_mask_op_tx(
        &mut self,
        layer_idx: usize,
        mask_idx: usize,
        op: MaskCombineOp,
        tx: &mut Tx,
    ) {
        if self.set_layer_mask_op(layer_idx, mask_idx, op) {
            self.sync_window_state(tx);
        }
    }

    pub fn toggle_layer_mask_invert_tx(&mut self, layer_idx: usize, mask_idx: usize, tx: &mut Tx) {
        if layer_idx >= self.recipe.layers.len()
            || mask_idx >= self.recipe.layers[layer_idx].masks.len()
        {
            return;
        }
        let mut next = self.recipe.clone();
        next.layers[layer_idx].masks[mask_idx].inverted =
            !next.layers[layer_idx].masks[mask_idx].inverted;
        self.commit_recipe_change(next, "Toggled mask item polarity");
        self.sync_window_state(tx);
    }

    pub fn remove_layer_mask_tx(&mut self, layer_idx: usize, mask_idx: usize, tx: &mut Tx) {
        if layer_idx >= self.recipe.layers.len()
            || mask_idx >= self.recipe.layers[layer_idx].masks.len()
        {
            return;
        }
        let mut next = self.recipe.clone();
        next.layers[layer_idx].masks.remove(mask_idx);
        self.commit_recipe_change(next, "Removed mask from layer");
        self.sync_window_state(tx);
    }

    #[cfg(test)]
    pub fn add_heal_spot(
        &mut self,
        target: [f32; 2],
        source: Option<[f32; 2]>,
        mode: HealMode,
    ) -> Option<usize> {
        if self.recipe.heal_spots.len() >= MAX_HEAL_SPOTS {
            self.status_message =
                Some(format!("Maximum {MAX_HEAL_SPOTS} heal/clone spots reached"));
            return None;
        }
        let id = self.next_heal_spot_id();
        let radius = self.heal_radius.clamp(0.004, 0.15);
        let src = source.unwrap_or_else(|| HealSpot::auto_source_for_target(target, radius));
        let spot = HealSpot {
            id,
            target,
            source: src,
            radius,
            feather: self.heal_feather.clamp(0.0, 1.0),
            opacity: self.heal_opacity.clamp(0.0, 1.0),
            mode,
        }
        .sanitized();
        let mut next = self.recipe.clone();
        next.heal_spots.push(spot);
        next.preset_name = None;
        let idx = next.heal_spots.len() - 1;
        self.commit_recipe_change(next, format!("Placed {} spot #{}", mode.label(), idx + 1));
        Some(idx)
    }

    pub fn remove_heal_spot_tx(&mut self, idx: usize, tx: &mut Tx) {
        if idx >= self.recipe.heal_spots.len() {
            return;
        }
        let mut next = self.recipe.clone();
        next.heal_spots.remove(idx);
        self.commit_recipe_change(next, format!("Removed retouch spot #{}", idx + 1));
        self.sync_window_state(tx);
    }

    pub fn clear_heal_spots(&mut self) {
        if self.recipe.heal_spots.is_empty() {
            return;
        }
        let mut next = self.recipe.clone();
        next.heal_spots.clear();
        self.commit_recipe_change(next, "Cleared all Heal & Clone spots");
    }

    pub fn clear_heal_spots_tx(&mut self, tx: &mut Tx) {
        self.clear_heal_spots();
        self.sync_window_state(tx);
    }

    /// Interactive canvas drag start for AI Select, Brush, Eraser, Linear/Radial Gradient, and Heal/Clone.
    pub fn begin_canvas_tool_drag(&mut self, local_x: f32, local_y: f32) {
        let Some([su, sv]) = self.viewport_local_to_source_uv(local_x, local_y) else {
            return;
        };
        if self.active_tool == CanvasTool::AiSelect {
            self.layer_drag = Some(LayerDragState::AiSelect {
                start_uv: [su, sv],
                current_uv: [su, sv],
                start_local: [local_x, local_y],
                shift_held: self.current_mods.shift,
                alt_held: self.current_mods.alt,
                dragging_box: false,
            });
            self.ai_select_bbox_preview = None;
            return;
        }
        if self.drag_undo_snapshot.is_none() {
            self.drag_undo_snapshot = Some(self.recipe.clone());
        }
        match self.active_tool {
            CanvasTool::Hand | CanvasTool::AiSelect => {}
            CanvasTool::Brush | CanvasTool::Eraser => {
                let erase = self.active_tool == CanvasTool::Eraser;
                let layer_idx = match self.active_layer_index {
                    Some(i) if i < self.recipe.layers.len() => i,
                    _ => {
                        if self.recipe.layers.len() >= MAX_ADJUSTMENT_LAYERS {
                            self.drag_undo_snapshot = None;
                            return;
                        }
                        let id = self.next_layer_id();
                        let n = self.recipe.layers.len() + 1;
                        self.recipe
                            .layers
                            .push(AdjustmentLayer::new(id, format!("Brush Layer {n}")));
                        let idx = self.recipe.layers.len() - 1;
                        self.active_layer_index = Some(idx);
                        idx
                    }
                };
                let mask_idx = if let Some(mi) = self.recipe.layers[layer_idx]
                    .masks
                    .iter()
                    .rposition(|m| matches!(m.shape, MaskShape::BrushStrokes { .. }))
                {
                    mi
                } else if self.recipe.layers[layer_idx].masks.len() < MAX_MASKS_PER_LAYER {
                    self.recipe.layers[layer_idx].masks.push(LayerMaskItem::new(
                        MaskShape::BrushStrokes {
                            strokes: Vec::new(),
                        },
                    ));
                    self.recipe.layers[layer_idx].masks.len() - 1
                } else {
                    self.drag_undo_snapshot = None;
                    return;
                };
                let dab = BrushDab {
                    x: su,
                    y: sv,
                    radius: self.brush_radius,
                    hardness: self.brush_hardness,
                    opacity: self.brush_opacity,
                    erase,
                }
                .sanitized();
                if let MaskShape::BrushStrokes { strokes } =
                    &mut self.recipe.layers[layer_idx].masks[mask_idx].shape
                    && strokes.len() < MAX_BRUSH_DABS_PER_MASK
                {
                    strokes.push(dab);
                }
                self.layer_drag = Some(LayerDragState::Brush {
                    layer_idx,
                    mask_idx,
                    erase,
                });
                self.sync_active_layer_camera();
            }
            CanvasTool::LinearGradient => {
                let layer_idx = match self.active_layer_index {
                    Some(i) if i < self.recipe.layers.len() => i,
                    _ => {
                        if self.recipe.layers.len() >= MAX_ADJUSTMENT_LAYERS {
                            self.drag_undo_snapshot = None;
                            return;
                        }
                        let id = self.next_layer_id();
                        let n = self.recipe.layers.len() + 1;
                        self.recipe
                            .layers
                            .push(AdjustmentLayer::new(id, format!("Linear Gradient {n}")));
                        let idx = self.recipe.layers.len() - 1;
                        self.active_layer_index = Some(idx);
                        idx
                    }
                };
                let shape = MaskShape::LinearGradient {
                    start: [su, sv],
                    end: [su, (sv + 0.02).min(1.0)],
                    feather: 0.75,
                };
                let mask_idx = if let Some(mi) = self.recipe.layers[layer_idx]
                    .masks
                    .iter()
                    .rposition(|m| matches!(m.shape, MaskShape::LinearGradient { .. }))
                {
                    self.recipe.layers[layer_idx].masks[mi].shape = shape;
                    mi
                } else if self.recipe.layers[layer_idx].masks.len() < MAX_MASKS_PER_LAYER {
                    self.recipe.layers[layer_idx]
                        .masks
                        .push(LayerMaskItem::new(shape));
                    self.recipe.layers[layer_idx].masks.len() - 1
                } else {
                    self.drag_undo_snapshot = None;
                    return;
                };
                self.layer_drag = Some(LayerDragState::Linear {
                    layer_idx,
                    mask_idx,
                    start_uv: [su, sv],
                });
                self.sync_active_layer_camera();
            }
            CanvasTool::RadialGradient => {
                let layer_idx = match self.active_layer_index {
                    Some(i) if i < self.recipe.layers.len() => i,
                    _ => {
                        if self.recipe.layers.len() >= MAX_ADJUSTMENT_LAYERS {
                            self.drag_undo_snapshot = None;
                            return;
                        }
                        let id = self.next_layer_id();
                        let n = self.recipe.layers.len() + 1;
                        self.recipe
                            .layers
                            .push(AdjustmentLayer::new(id, format!("Radial Gradient {n}")));
                        let idx = self.recipe.layers.len() - 1;
                        self.active_layer_index = Some(idx);
                        idx
                    }
                };
                let shape = MaskShape::RadialGradient {
                    center: [su, sv],
                    radius: [0.05, 0.05],
                    angle_deg: 0.0,
                    feather: 0.45,
                };
                let mask_idx = if let Some(mi) = self.recipe.layers[layer_idx]
                    .masks
                    .iter()
                    .rposition(|m| matches!(m.shape, MaskShape::RadialGradient { .. }))
                {
                    self.recipe.layers[layer_idx].masks[mi].shape = shape;
                    mi
                } else if self.recipe.layers[layer_idx].masks.len() < MAX_MASKS_PER_LAYER {
                    self.recipe.layers[layer_idx]
                        .masks
                        .push(LayerMaskItem::new(shape));
                    self.recipe.layers[layer_idx].masks.len() - 1
                } else {
                    self.drag_undo_snapshot = None;
                    return;
                };
                self.layer_drag = Some(LayerDragState::Radial {
                    layer_idx,
                    mask_idx,
                    center_uv: [su, sv],
                });
                self.sync_active_layer_camera();
            }
            CanvasTool::Heal | CanvasTool::Clone => {
                if self.recipe.heal_spots.len() >= MAX_HEAL_SPOTS {
                    self.drag_undo_snapshot = None;
                    self.status_message =
                        Some(format!("Maximum {MAX_HEAL_SPOTS} heal/clone spots reached"));
                    return;
                }
                let mode = if self.active_tool == CanvasTool::Clone {
                    HealMode::Clone
                } else {
                    HealMode::Heal
                };
                let id = self.next_heal_spot_id();
                let radius = self.heal_radius.clamp(0.004, 0.15);
                let spot = HealSpot {
                    id,
                    target: [su, sv],
                    source: HealSpot::auto_source_for_target([su, sv], radius),
                    radius,
                    feather: self.heal_feather.clamp(0.0, 1.0),
                    opacity: self.heal_opacity.clamp(0.0, 1.0),
                    mode,
                }
                .sanitized();
                self.recipe.heal_spots.push(spot);
                let spot_idx = self.recipe.heal_spots.len() - 1;
                self.layer_drag = Some(LayerDragState::Heal { spot_idx });
            }
        }
    }

    pub fn update_canvas_tool_drag(&mut self, local_x: f32, local_y: f32) {
        let Some(drag) = self.layer_drag else {
            return;
        };
        let Some([su, sv]) = self.viewport_local_to_source_uv(local_x, local_y) else {
            return;
        };
        match drag {
            LayerDragState::AiSelect {
                start_uv,
                start_local,
                shift_held,
                alt_held,
                dragging_box,
                ..
            } => {
                let shift_now = shift_held || self.current_mods.shift;
                let moved = (local_x - start_local[0]).hypot(local_y - start_local[1]);
                let is_box = dragging_box || (shift_now && moved >= 4.0);
                self.layer_drag = Some(LayerDragState::AiSelect {
                    start_uv,
                    current_uv: [su, sv],
                    start_local,
                    shift_held: shift_now,
                    alt_held,
                    dragging_box: is_box,
                });
                if is_box {
                    self.ai_select_bbox_preview =
                        Some(PromptBox::new(start_uv[0], start_uv[1], su, sv).sanitized());
                }
            }
            LayerDragState::Brush {
                layer_idx,
                mask_idx,
                erase,
            } => {
                if let Some(layer) = self.recipe.layers.get_mut(layer_idx)
                    && let Some(mask) = layer.masks.get_mut(mask_idx)
                    && let MaskShape::BrushStrokes { strokes } = &mut mask.shape
                {
                    let spacing = (self.brush_radius * 0.25).max(0.004);
                    let far_enough = strokes.last().is_none_or(|prev| {
                        let dx = su - prev.x;
                        let dy = sv - prev.y;
                        (dx * dx + dy * dy).sqrt() >= spacing
                    });
                    if far_enough && strokes.len() < MAX_BRUSH_DABS_PER_MASK {
                        strokes.push(
                            BrushDab {
                                x: su,
                                y: sv,
                                radius: self.brush_radius,
                                hardness: self.brush_hardness,
                                opacity: self.brush_opacity,
                                erase,
                            }
                            .sanitized(),
                        );
                    }
                }
            }
            LayerDragState::Linear {
                layer_idx,
                mask_idx,
                start_uv,
            } => {
                if let Some(layer) = self.recipe.layers.get_mut(layer_idx)
                    && let Some(mask) = layer.masks.get_mut(mask_idx)
                    && let MaskShape::LinearGradient { start, end, .. } = &mut mask.shape
                {
                    *start = start_uv;
                    *end = [su, sv];
                }
            }
            LayerDragState::Radial {
                layer_idx,
                mask_idx,
                center_uv,
            } => {
                if let Some(layer) = self.recipe.layers.get_mut(layer_idx)
                    && let Some(mask) = layer.masks.get_mut(mask_idx)
                    && let MaskShape::RadialGradient { center, radius, .. } = &mut mask.shape
                {
                    *center = center_uv;
                    radius[0] = (su - center_uv[0]).abs().clamp(0.02, 1.0);
                    radius[1] = (sv - center_uv[1]).abs().clamp(0.02, 1.0);
                }
            }
            LayerDragState::Heal { spot_idx } => {
                if let Some(spot) = self.recipe.heal_spots.get_mut(spot_idx) {
                    spot.source = [su.clamp(0.0, 1.0), sv.clamp(0.0, 1.0)];
                }
            }
        }
    }

    pub fn finish_canvas_tool_drag(&mut self) {
        if let Some(LayerDragState::AiSelect {
            start_uv,
            current_uv,
            shift_held,
            alt_held,
            dragging_box,
            ..
        }) = self.layer_drag.take()
        {
            self.ai_select_bbox_preview = None;
            let candidate_box =
                PromptBox::new(start_uv[0], start_uv[1], current_uv[0], current_uv[1]).sanitized();
            if (dragging_box || shift_held) && candidate_box.is_valid() {
                self.ai_select_set_bbox(candidate_box);
            } else {
                let positive = self.ai_select_include_mode && !alt_held;
                self.ai_select_add_point(start_uv[0], start_uv[1], positive);
            }
            return;
        }
        self.layer_drag = None;
        self.recipe.preset_name = None;
        self.catalog.update_active_recipe(self.recipe.clone());
        if let Some(prev) = self.drag_undo_snapshot.take()
            && prev != self.recipe
        {
            self.history.push(prev);
        }
        self.recompute_histogram();
    }

    pub fn end_canvas_tool_drag_tx(&mut self, tx: &mut Tx) {
        self.finish_canvas_tool_drag();
        self.sync_window_state(tx);
    }

    /// Projects a source-image UV `[su, sv]` (`0..=1`) into viewport local coordinates (points)
    /// for crisp Capture One vector canvas guides (Linear/Radial Gradient & Heal/Clone pins).
    pub fn source_uv_to_viewport_local(&self, uv: [f32; 2], vw_pts: f32, vh_pts: f32) -> [f32; 2] {
        let scale = self.viewport_scale.max(1.0);
        let slot_w = (vw_pts * scale).max(1.0);
        let slot_h = (vh_pts * scale).max(1.0);
        let geo = &self.recipe.geometry;
        let [ori_w, ori_h] = geo.oriented_size(self.image.width, self.image.height);
        let [cx, cy, cw, ch] = geo.sanitized_crop();

        // Un-rotate first via quarter_turns(), then un-flip (exact inverse of oriented_to_source_uv_unclamped).
        let (mut ou, mut ov) = match geo.quarter_turns() {
            1 => (1.0 - uv[1], uv[0]),
            2 => (1.0 - uv[0], 1.0 - uv[1]),
            3 => (uv[1], 1.0 - uv[0]),
            _ => (uv[0], uv[1]),
        };
        if geo.flip_h {
            ou = 1.0 - ou;
        }
        if geo.flip_v {
            ov = 1.0 - ov;
        }

        let (frame_w, frame_h, frame_u, frame_v) = if self.camera.crop_mode {
            (ori_w as f32, ori_h as f32, ou, ov)
        } else {
            (
                (ori_w as f32 * cw).max(1.0),
                (ori_h as f32 * ch).max(1.0),
                (ou - cx) / cw.max(1e-4),
                (ov - cy) / ch.max(1e-4),
            )
        };

        let pad = 28.0_f32;
        let avail_w = (slot_w - pad * 2.0).max(1.0);
        let avail_h = (slot_h - pad * 2.0).max(1.0);
        let fit_scale = (avail_w / frame_w).min(avail_h / frame_h);
        let fitted_w = frame_w * fit_scale;
        let fitted_h = frame_h * fit_scale;

        let zoom = self.camera.zoom.max(0.05);
        let pan_x = self.camera.pan[0] * slot_w * 0.5;
        let pan_y = self.camera.pan[1] * slot_h * 0.5;
        let (sn, cs) = self.camera.rotation_deg.to_radians().sin_cos();

        let rx = (frame_u - 0.5) * fitted_w * zoom;
        let ry = (frame_v - 0.5) * fitted_h * zoom;
        let px = rx * cs - ry * sn + pan_x;
        let py = rx * sn + ry * cs + pan_y;

        [(px / slot_w + 0.5) * vw_pts, (py / slot_h + 0.5) * vh_pts]
    }

    // -------------------------------------------------------------------------
    // Phase 5: Film Negative Conversion, Proof Margin, 4-Folder Sessions,
    // Hot Folder Auto-Ingest, External Editor Handoff & Custom Shortcuts
    // -------------------------------------------------------------------------

    pub fn set_processing_mode(&mut self, mode: ProcessingMode) {
        if self.recipe.processing_mode == mode {
            return;
        }
        let mut next = self.recipe.clone();
        next.processing_mode = mode;
        next.preset_name = None;
        if mode != ProcessingMode::FilmNegative {
            self.film_base_picker_active = false;
        }
        self.commit_recipe_change(next, format!("Processing mode: {}", mode.label()));
    }

    pub fn set_processing_mode_tx(&mut self, mode: ProcessingMode, tx: &mut Tx) {
        self.set_processing_mode(mode);
        self.sync_window_state(tx);
    }

    pub fn toggle_film_base_picker_tx(&mut self, tx: &mut Tx) {
        self.film_base_picker_active = !self.film_base_picker_active;
        if self.film_base_picker_active {
            self.wb_eyedropper_active = false;
            self.color_slice_picker_active = false;
            self.skin_picker_active = false;
            self.readout_picker_active = false;
            self.active_tool = CanvasTool::Hand;
            self.camera.crop_mode = false;
            self.active_tab = InspectorTab::Light;
            self.film_neg_open = true;
            self.status_message =
                Some("Film Base Picker — click unexposed orange film border on photo".to_owned());
        } else {
            self.status_message = Some("Cancelled Film Base Picker".to_owned());
        }
        self.sync_window_state(tx);
    }

    /// Samples a 3×3 raw pre-grade linear patch under the viewport click, switches to
    /// `ProcessingMode::FilmNegative`, and calibrates the C-41 orange film base mask.
    pub fn pick_film_base_at_viewport(&mut self, local_x: f32, local_y: f32) -> bool {
        let Some([su, sv]) = self.viewport_local_to_source_uv(local_x, local_y) else {
            self.film_base_picker_active = false;
            self.status_message = Some("Click inside the photo to sample the film base".to_owned());
            return false;
        };
        let avg = self.sample_raw_patch_linear(su, sv, 1);
        let mut next = self.recipe.clone();
        let base = next.convert_film_negative_from_base(avg);
        next.preset_name = None;
        self.film_base_picker_active = false;
        self.commit_recipe_change(
            next,
            format!(
                "Converted Film Negative (Base RGB {:.2}, {:.2}, {:.2})",
                base[0], base[1], base[2]
            ),
        );
        true
    }

    pub fn pick_film_base_at_viewport_tx(&mut self, local_x: f32, local_y: f32, tx: &mut Tx) {
        self.pick_film_base_at_viewport(local_x, local_y);
        self.sync_window_state(tx);
    }

    pub fn convert_film_negative_auto_tx(&mut self, tx: &mut Tx) {
        // Sample near top-left border (typical film rebate / neutral border) with safe fallback
        let avg = self.sample_raw_patch_linear(0.03, 0.03, 2);
        let mut next = self.recipe.clone();
        let base = next.convert_film_negative_from_base(avg);
        next.preset_name = None;
        self.commit_recipe_change(
            next,
            format!(
                "Converted Film Negative (Auto Base {:.2}, {:.2}, {:.2})",
                base[0], base[1], base[2]
            ),
        );
        self.sync_window_state(tx);
    }

    pub fn reset_film_negative_tx(&mut self, tx: &mut Tx) {
        let mut next = self.recipe.clone();
        next.film_negative = FilmNegativeParams::default();
        next.preset_name = None;
        self.commit_recipe_change(next, "Reset Film Negative conversion parameters");
        self.sync_window_state(tx);
    }

    pub fn set_proof_margin(&mut self, margin: ProofMargin) {
        self.proof_margin = margin;
        self.save_app_settings();
        self.status_message = Some(format!(
            "Proof Margin: {} ({})",
            margin.label(),
            margin.effective_pt_label()
        ));
    }

    pub fn set_proof_margin_tx(&mut self, margin: ProofMargin, tx: &mut Tx) {
        self.set_proof_margin(margin);
        self.sync_window_state(tx);
    }

    pub fn cycle_proof_margin(&mut self) {
        let next = self.proof_margin.next();
        self.set_proof_margin(next);
    }

    pub fn cycle_proof_margin_tx(&mut self, tx: &mut Tx) {
        self.cycle_proof_margin();
        self.sync_window_state(tx);
    }

    /// Creates a Capture One 4-folder session (`Capture/`, `Selects/`, `Output/`, `Trash/`)
    /// and opens `Capture/` as the active session folder.
    pub fn create_capture_one_session_tx(&mut self, root: PathBuf, tx: &mut Tx) {
        match create_capture_one_session(&root) {
            Ok(layout) => {
                self.open_folder_tx(layout.capture.clone(), tx);
                let _ = self.start_hot_folder_watch(layout.capture.clone());
                self.status_message = Some(format!(
                    "Created Capture One Session (Capture/Selects/Output/Trash) in {}",
                    root.display()
                ));
                self.sync_window_state(tx);
            }
            Err(err) => {
                self.status_message = Some(format!("Create session failed: {err}"));
            }
        }
    }

    /// Safely moves the active photo + `.rune-edit.json` sidecar into `Selects/` (`⌘J`)
    /// without ever clobbering existing files.
    pub fn move_active_to_selects_tx(&mut self, tx: &mut Tx) {
        if self.demo_mode {
            self.status_message = Some("Open a photo from disk to move it to Selects/".to_owned());
            return;
        }
        let Some(photo_path) = self.image.source_path.clone() else {
            self.status_message = Some("No disk photo active to move to Selects/".to_owned());
            return;
        };
        let Some(parent) = photo_path.parent() else {
            return;
        };
        let selects_dir = if parent
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("Capture"))
        {
            parent
                .parent()
                .map(|root| root.join("Selects"))
                .unwrap_or_else(|| parent.join("Selects"))
        } else {
            parent.join("Selects")
        };

        if let Some(idx) = self.catalog.active_index {
            self.autosave_dirty_entry_sidecar(idx);
        }
        match move_photo_with_sidecar(&photo_path, &selects_dir) {
            Ok(dest) => {
                let name = dest
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "photo".to_owned());
                if let Some(idx) = self.catalog.active_index
                    && idx < self.catalog.entries.len()
                {
                    self.catalog.entries.remove(idx);
                    if self.catalog.entries.is_empty() {
                        self.catalog.active_index = None;
                    } else {
                        self.catalog.active_index = Some(idx.min(self.catalog.entries.len() - 1));
                        self.load_active_catalog_photo_tx(tx);
                    }
                }
                self.status_message = Some(format!("Moved {name} + sidecar → Selects/"));
                self.sync_window_state(tx);
            }
            Err(err) => {
                self.status_message = Some(format!("Move to Selects failed: {err}"));
            }
        }
    }

    /// Non-destructively exports a full-resolution 16-bit TIFF sibling (`<stem>_edit.tif`)
    /// without ever touching the original RAW, then opens it in macOS's default external editor.
    pub fn edit_with_external_editor_tx(&mut self, tx: &mut Tx) {
        let Some(src) = self.image.source_path.clone() else {
            self.status_message =
                Some("Open a photo from disk to use Edit With External Editor…".to_owned());
            return;
        };
        let parent = src.parent().unwrap_or_else(|| Path::new("."));
        let stem = src
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "photo".to_owned());
        let mut dest = parent.join(format!("{stem}_edit.tif"));
        let mut suffix = 2u32;
        while dest.exists() || dest.symlink_metadata().is_ok() {
            dest = parent.join(format!("{stem}_edit_{suffix}.tif"));
            suffix += 1;
        }
        if let Err(err) = guard_not_source(&src, &dest) {
            self.status_message = Some(format!("External editor guard: {err}"));
            return;
        }
        let opts = ExportOptions {
            format: ExportFormat::Tiff16,
            color_space: ExportColorSpace::DisplayP3,
            respect_crop: true,
            ..ExportOptions::default()
        };
        match export_image_with_options(&self.image, &self.recipe, &dest, &opts) {
            Ok(()) => {
                #[cfg(target_os = "macos")]
                let launch = Command::new("open").arg(&dest).spawn();
                #[cfg(not(target_os = "macos"))]
                let launch: std::io::Result<std::process::Child> = Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "external launch supported on macOS",
                ));
                let fname = dest
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "edit.tif".to_owned());
                match launch {
                    Ok(_) => {
                        self.status_message =
                            Some(format!("Exported 16-bit TIFF {fname} → opened externally"));
                    }
                    Err(err) => {
                        self.status_message =
                            Some(format!("Exported {fname} (open failed: {err})"));
                    }
                }
                self.sync_window_state(tx);
            }
            Err(err) => {
                self.status_message = Some(format!("External edit export failed: {err}"));
            }
        }
    }

    pub fn start_hot_folder_watch(&mut self, dir: PathBuf) -> Result<(), String> {
        let watcher = HotFolderWatcher::new(dir.clone()).map_err(|e| e.to_string())?;
        self.hot_folder_watcher = Some(watcher);
        self.hot_folder_dir = Some(dir.clone());
        let name = dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| dir.display().to_string());
        self.status_message = Some(format!("Hot Folder watching '{name}' for new shots"));
        Ok(())
    }

    pub fn start_hot_folder_watch_tx(&mut self, dir: PathBuf, tx: &mut Tx) {
        match self.start_hot_folder_watch(dir) {
            Ok(()) => self.sync_window_state(tx),
            Err(err) => self.status_message = Some(format!("Hot Folder error: {err}")),
        }
    }

    pub fn stop_hot_folder_watch(&mut self) {
        self.hot_folder_watcher = None;
        self.hot_folder_dir = None;
        self.status_message = Some("Hot Folder watch stopped".to_owned());
    }

    pub fn stop_hot_folder_watch_tx(&mut self, tx: &mut Tx) {
        self.stop_hot_folder_watch();
        self.sync_window_state(tx);
    }

    /// Polls the active `HotFolderWatcher` for newly stable photos, ingests them into the
    /// session catalog, and optionally applies `hot_folder_style` + saves their sidecars.
    pub fn poll_hot_folder(&mut self) -> usize {
        let ready = {
            let Some(watcher) = self.hot_folder_watcher.as_mut() else {
                return 0;
            };
            match watcher.poll_ready() {
                Ok(files) => files,
                Err(_) => return 0,
            }
        };
        if ready.is_empty() {
            return 0;
        }
        let mut ingested = 0usize;
        for path in ready {
            if self.catalog.entries.iter().any(|e| e.path == path) {
                continue;
            }
            let mut meta = PhotoMeta::load_sidecar(&path)
                .ok()
                .flatten()
                .unwrap_or_default();
            if let Some(style_id) = self.hot_folder_style.as_deref()
                && let Some(preset) = self.presets.iter().find(|p| p.id == style_id)
            {
                meta.recipe = preset.recipe.clone();
                let _ = meta.save_sidecar_in_place(&path);
            }
            self.catalog.entries.push(SessionEntry::new(path, meta));
            ingested += 1;
        }
        if ingested > 0 {
            self.demo_mode = false;
            self.hot_folder_ingested_count += ingested;
            self.status_message = Some(format!(
                "Hot Folder ingested {ingested} new photo(s) ({} total)",
                self.hot_folder_ingested_count
            ));
        }
        ingested
    }

    pub fn poll_hot_folder_tx(&mut self, tx: &mut Tx) {
        let n = self.poll_hot_folder();
        if n > 0 {
            self.populate_visible_thumbnails(24);
            if let Some(last) = self.catalog.entries.len().checked_sub(1) {
                self.catalog.select_index(last);
                self.load_active_catalog_photo_tx(tx);
            }
        } else {
            self.status_message = Some("Hot Folder: no new shots ready yet".to_owned());
        }
        self.sync_window_state(tx);
    }

    pub fn set_hot_folder_style_tx(&mut self, style_id: Option<String>, tx: &mut Tx) {
        self.hot_folder_style = style_id;
        self.save_app_settings();
        self.status_message = Some(match self.hot_folder_style.as_deref() {
            Some(id) => format!("Hot Folder auto-apply style: {id}"),
            None => "Hot Folder auto-apply style: None".to_owned(),
        });
        self.sync_window_state(tx);
    }

    pub fn set_shortcut_binding(
        &mut self,
        action: ShortcutAction,
        key: &str,
    ) -> Result<(), String> {
        self.shortcuts.set_key(action, key)?;
        self.recording_shortcut = None;
        self.save_app_settings();
        self.status_message = Some(format!(
            "Bound '{}' → {}",
            action.label(),
            self.shortcuts.display_key(action)
        ));
        Ok(())
    }

    pub fn set_shortcut_binding_tx(
        &mut self,
        action: ShortcutAction,
        key: &str,
        tx: &mut Tx,
    ) -> Result<(), String> {
        let res = self.set_shortcut_binding(action, key);
        if res.is_ok() {
            self.sync_window_state(tx);
        } else if let Err(ref err) = res {
            self.status_message = Some(err.clone());
        }
        res
    }

    pub fn reset_shortcut_defaults(&mut self) {
        self.shortcuts.reset_defaults();
        self.recording_shortcut = None;
        self.save_app_settings();
        self.status_message = Some("Reset keyboard shortcuts to Capture One defaults".to_owned());
    }

    pub fn reset_shortcut_defaults_tx(&mut self, tx: &mut Tx) {
        self.reset_shortcut_defaults();
        self.sync_window_state(tx);
    }

    pub fn clear_compare_variant_tx(&mut self, tx: &mut Tx) {
        if self.compare_reference.take().is_some() {
            self.status_message = Some("Cleared Compare Variant reference".to_owned());
            self.sync_window_state(tx);
        }
    }

    pub fn toggle_clipboard_group(&mut self, group: &str) {
        if !self.clipboard_groups.remove(group) {
            self.clipboard_groups.insert(group.to_owned());
        }
    }

    pub fn autoselect_adjusted_groups(&mut self) {
        let adjusted = modified_recipe_groups(&self.recipe);
        self.clipboard_groups = adjusted.into_iter().map(str::to_owned).collect();
        self.status_message = Some(format!(
            "Autoselected {} adjusted group(s)",
            self.clipboard_groups.len()
        ));
    }

    pub fn paste_selected_adjustments(&mut self) -> bool {
        let Some(copied) = self.catalog.paste_adjustments() else {
            self.status_message = Some("No adjustments copied yet".to_owned());
            return false;
        };
        let groups: Vec<String> = self.clipboard_groups.iter().cloned().collect();
        if groups.is_empty() {
            self.status_message = Some("Select at least one clipboard group to apply".to_owned());
            return false;
        }
        let mut next = self.recipe.clone();
        if copy_recipe_groups(&mut next, &copied, &groups).is_ok() {
            next.preset_name = None;
            let count = groups.len();
            self.commit_recipe_change(
                next,
                format!("Applied {count} selected adjustment group(s)"),
            );
            true
        } else {
            false
        }
    }

    pub fn paste_selected_adjustments_tx(&mut self, tx: &mut Tx) {
        if self.paste_selected_adjustments() {
            self.sync_window_state(tx);
            tx.announce("Applied selected adjustments", LiveRegion::Polite);
        }
    }

    pub fn save_current_as_user_style(&mut self, raw_name: &str) -> Result<(), String> {
        let name = raw_name.trim();
        let groups: Vec<String> = self.clipboard_groups.iter().cloned().collect();
        let style = UserStyle::from_recipe(
            name,
            format!("Saved from {}", self.image.display_name),
            &self.recipe,
            &groups,
        )
        .map_err(|e| e.to_string())?;
        save_user_style(&style).map_err(|e| e.to_string())?;
        self.user_styles = list_user_styles().unwrap_or_else(|_| vec![style]);
        self.style_name_edit.clear();
        self.status_message = Some(format!("Saved User Style: {name}"));
        Ok(())
    }

    pub fn save_current_as_user_style_tx(&mut self, tx: &mut Tx) {
        let name = self.style_name_edit.text.clone();
        match self.save_current_as_user_style(&name) {
            Ok(()) => self.sync_window_state(tx),
            Err(err) => self.status_message = Some(format!("Save Style failed: {err}")),
        }
    }

    pub fn apply_user_style(&mut self, idx: usize) -> bool {
        let Some(style) = self.user_styles.get(idx).cloned() else {
            return false;
        };
        let strength = self.style_strength.clamp(0.0, 1.0);
        let mut next = self.recipe.clone();
        let res = if self.stack_styles {
            stack_style_onto_recipe(&mut next, &style.patch, strength)
        } else {
            apply_style_to_recipe(&mut next, &style, strength)
        };
        if res.is_ok() {
            next.preset_name = None;
            let verb = if self.stack_styles {
                "Stacked"
            } else {
                "Applied"
            };
            self.commit_recipe_change(
                next,
                format!(
                    "{verb} User Style: {} ({:.0}%)",
                    style.name,
                    strength * 100.0
                ),
            );
            true
        } else {
            false
        }
    }

    pub fn apply_user_style_tx(&mut self, idx: usize, tx: &mut Tx) {
        if self.apply_user_style(idx) {
            self.sync_window_state(tx);
        }
    }

    pub fn delete_user_style_tx(&mut self, idx: usize, tx: &mut Tx) {
        let Some(style) = self.user_styles.get(idx).cloned() else {
            return;
        };
        if let Ok(slug) = style.slug() {
            let _ = delete_user_style(&slug);
        }
        self.user_styles.remove(idx);
        self.status_message = Some(format!("Deleted User Style: {}", style.name));
        self.sync_window_state(tx);
    }

    pub fn toggle_loupe_tx(&mut self, tx: &mut Tx) {
        self.camera.loupe_enabled = !self.camera.loupe_enabled;
        if self.camera.loupe_enabled {
            self.show_grid_browser = false;
            self.camera.crop_mode = false;
            self.status_message = Some(format!(
                "120Hz GPU Loupe ON ({:.1}×) — move pointer to inspect",
                self.camera.loupe_zoom
            ));
        } else {
            self.status_message = Some("GPU Loupe OFF".to_owned());
        }
        self.sync_window_state(tx);
    }

    pub fn update_loupe_uv(&mut self, local_x: f32, local_y: f32) {
        let w = self.viewport_rect_pts[0];
        let h = self.viewport_rect_pts[1];
        if w > 1.0 && h > 1.0 {
            self.camera.loupe_uv = [(local_x / w).clamp(0.0, 1.0), (local_y / h).clamp(0.0, 1.0)];
        }
    }

    pub fn toggle_readout_picker_tx(&mut self, tx: &mut Tx) {
        self.readout_picker_active = !self.readout_picker_active;
        if self.readout_picker_active {
            self.wb_eyedropper_active = false;
            self.color_slice_picker_active = false;
            self.skin_picker_active = false;
            self.camera.crop_mode = false;
            self.status_message =
                Some("Pin Readout — click photo to pin live RGB/Lab marker (#1–#4)".to_owned());
        } else {
            self.status_message = Some("Pin Readout tool OFF".to_owned());
        }
        self.sync_window_state(tx);
    }

    pub fn pin_color_readout_at_viewport(&mut self, local_x: f32, local_y: f32) -> bool {
        let w = self.viewport_rect_pts[0];
        let h = self.viewport_rect_pts[1];
        if w <= 1.0 || h <= 1.0 {
            return false;
        }
        let Some(source_uv) = self.viewport_local_to_source_uv(local_x, local_y) else {
            return false;
        };
        let slot_uv = [(local_x / w).clamp(0.0, 1.0), (local_y / h).clamp(0.0, 1.0)];
        if self.pinned_readouts.len() >= MAX_PINNED_READOUTS {
            self.pinned_readouts.remove(0);
        }
        self.pinned_readouts
            .push(PinnedReadout { source_uv, slot_uv });
        self.status_message = Some(format!(
            "Pinned Color Readout #{} (live graded RGB/Lab)",
            self.pinned_readouts.len()
        ));
        true
    }

    pub fn pin_color_readout_at_viewport_tx(&mut self, local_x: f32, local_y: f32, tx: &mut Tx) {
        if self.pin_color_readout_at_viewport(local_x, local_y) {
            self.sync_window_state(tx);
        }
    }

    pub fn evaluate_pinned_readout(&self, pin: &PinnedReadout) -> CursorColorReadout {
        let src_lin = self.sample_raw_pixel_linear(pin.source_uv[0], pin.source_uv[1]);
        let graded_lin = self.compiled_recipe.evaluate_pixel_linear(src_lin);
        let srgb = [
            linear_to_srgb(graded_lin[0]),
            linear_to_srgb(graded_lin[1]),
            linear_to_srgb(graded_lin[2]),
        ];
        let rgb8 = [
            (srgb[0] * 255.0).round().clamp(0.0, 255.0) as u8,
            (srgb[1] * 255.0).round().clamp(0.0, 255.0) as u8,
            (srgb[2] * 255.0).round().clamp(0.0, 255.0) as u8,
        ];
        let (hsl, lab) = rgb_to_hsl_and_lab(srgb);
        CursorColorReadout { rgb8, hsl, lab }
    }

    pub fn current_naming_context(&self, recipe_name: &str) -> NamingContext {
        let image_name = self
            .image
            .source_path
            .as_deref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Editorial_Portrait".to_owned());
        let (camera, lens, iso) = self.image.exif.as_ref().map_or_else(
            || {
                (
                    "Leica_Q3".to_owned(),
                    "28mm".to_owned(),
                    "ISO100".to_owned(),
                )
            },
            |ex| {
                (
                    ex.camera_model.clone().unwrap_or_default(),
                    ex.lens_model.clone().unwrap_or_default(),
                    ex.iso.map(|i| format!("ISO{i}")).unwrap_or_default(),
                )
            },
        );
        NamingContext {
            image_name,
            counter: self.catalog.active_index.map_or(1, |i| i + 1),
            date: "2026-11-26".to_owned(),
            camera,
            lens,
            iso,
            variant_index: self.active_variant_index(),
            recipe_name: recipe_name.to_owned(),
        }
    }

    pub fn preview_export_filename(&self, spec: &ExportRecipeSpec) -> String {
        let tpl = if self.export_naming_edit.text.trim().is_empty() {
            spec.naming_template.as_str()
        } else {
            self.export_naming_edit.text.as_str()
        };
        let ctx = self.current_naming_context(&spec.name);
        let stem = expand_naming_template(tpl, &ctx);
        format!("{stem}.{}", spec.options.format.extension())
    }

    pub fn run_multi_recipe_export_tx(&mut self, out_dir: PathBuf, tx: &mut Tx) {
        self.export_output_dir = Some(out_dir.clone());
        let tpl = if self.export_naming_edit.text.trim().is_empty() {
            "{ImageName}_{Recipe}".to_owned()
        } else {
            self.export_naming_edit.text.clone()
        };
        let wm = TextWatermark {
            enabled: self.export_watermark_enabled,
            text: self.export_watermark_edit.text.clone(),
            opacity: self.export_watermark_opacity,
            scale: 1.0,
            position: self.export_watermark_position,
        };
        let recipes: Vec<ExportRecipeSpec> = self
            .export_recipes
            .iter()
            .cloned()
            .map(|mut r| {
                r.naming_template = tpl.clone();
                r.options.strip_metadata = self.export_strip_metadata;
                r.options.watermark = wm.clone();
                r.options.color_space = self.export_color_space;
                r
            })
            .collect();
        let ctx = self.current_naming_context("");
        match export_multi_recipe(
            &self.image,
            &self.recipe,
            &out_dir,
            &recipes,
            &ctx,
            self.image.source_path.as_deref(),
            false,
        ) {
            Ok(items) => {
                let msg = format!(
                    "Exported {} recipe(s) to {}",
                    items.len(),
                    out_dir.display()
                );
                self.status_message = Some(msg.clone());
                self.show_export = false;
                self.sync_window_state(tx);
                tx.announce(msg, LiveRegion::Polite);
            }
            Err(err) => {
                self.status_message = Some(format!("Multi-recipe export failed: {err}"));
            }
        }
    }

    pub fn reset_recipe(&mut self) {
        self.commit_recipe_change(DevelopRecipe::default(), "Reset all adjustments to Neutral");
    }

    pub fn reset_recipe_tx(&mut self, tx: &mut Tx) {
        self.reset_recipe();
        self.sync_window_state(tx);
        tx.announce("Reset all adjustments", LiveRegion::Polite);
    }

    pub fn reset_active_curve_channel_tx(&mut self, tx: &mut Tx) {
        let mut next = self.recipe.clone();
        *self.curve_channel.points_mut(&mut next.curves) = default_curve_points();
        next.preset_name = None;
        let label = format!("Reset {} curve", self.curve_channel.label());
        self.commit_recipe_change(next, label);
        self.sync_window_state(tx);
    }

    pub fn undo(&mut self) {
        self.clear_transient_interactions();
        if self.history.undo(&mut self.recipe) {
            self.catalog.update_active_recipe(self.recipe.clone());
            self.sync_profile_state_after_recipe_change();
            self.recompute_histogram();
            self.status_message = Some("Undid last adjustment".to_owned());
        }
    }

    pub fn undo_tx(&mut self, tx: &mut Tx) {
        self.undo();
        self.sync_window_state(tx);
    }

    pub fn redo(&mut self) {
        self.clear_transient_interactions();
        if self.history.redo(&mut self.recipe) {
            self.catalog.update_active_recipe(self.recipe.clone());
            self.sync_profile_state_after_recipe_change();
            self.recompute_histogram();
            self.status_message = Some("Redid adjustment".to_owned());
        }
    }

    pub fn redo_tx(&mut self, tx: &mut Tx) {
        self.redo();
        self.sync_window_state(tx);
    }

    pub fn copy_adjustments_tx(&mut self, tx: &mut Tx) {
        self.catalog.copy_adjustments(&self.recipe);
        self.status_message = Some("Copied develop adjustments to clipboard".to_owned());
        self.sync_native_menu();
        tx.announce("Copied adjustments", LiveRegion::Polite);
    }

    pub fn save_sidecar_tx(&mut self, tx: &mut Tx) {
        let Some(path) = self.image.source_path.as_deref() else {
            self.status_message =
                Some("Open a photo from disk to save a .rune-edit.json sidecar".to_owned());
            return;
        };
        let mut meta = self
            .catalog
            .active_meta()
            .cloned()
            .map(|mut m| {
                m.recipe = self.recipe.clone();
                m
            })
            .unwrap_or_else(|| PhotoMeta {
                recipe: self.recipe.clone(),
                ..PhotoMeta::default()
            });
        let diffs = rune_editor_core::recipe_field_diffs(&self.saved_recipe, &self.recipe);
        if !diffs.is_empty() {
            meta.push_history(
                rune_editor_core::EditActor::Human,
                format!("GUI save ({} field(s))", diffs.len()),
                diffs,
                Some(self.saved_recipe.clone()),
            );
        }
        match meta.save_sidecar_in_place(path) {
            Ok(sidecar) => {
                if let Some(active) = self.catalog.active_meta_mut() {
                    *active = meta;
                }
                self.saved_recipe = self.recipe.clone();
                let name = sidecar
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "sidecar".to_owned());
                self.status_message = Some(format!("Saved sidecar {name}"));
                self.sync_window_state(tx);
                tx.announce(format!("Saved sidecar {name}"), LiveRegion::Polite);
            }
            Err(err) => {
                self.status_message = Some(format!("Sidecar save failed: {err}"));
            }
        }
    }

    pub fn open_image_path_tx(&mut self, path: PathBuf, tx: &mut Tx) {
        let restored_meta = PhotoMeta::load_sidecar(&path)
            .ok()
            .flatten()
            .unwrap_or_default();
        let decode_mode = Self::raw_decode_mode_for_recipe(&restored_meta.recipe);
        let use_cache = decode_mode == RawDecodeMode::Linear || !is_raw_extension(&path);

        let cached = if use_cache {
            self.prefetch_cache
                .lock()
                .ok()
                .and_then(|c| c.get(&path).cloned())
        } else {
            None
        };

        let decoded_res = match cached {
            Some(img) => Ok(img),
            None => load_file_proxy_with_mode(&path, None, decode_mode).map(Arc::new),
        };

        match decoded_res {
            Ok(decoded) => {
                let name = decoded.display_name.clone();
                let dims = format!("{}×{}", decoded.width, decoded.height);

                if self.demo_mode || !self.catalog.entries.iter().any(|e| e.path == path) {
                    self.demo_mode = false;
                    let parent = path.parent().map(Path::to_path_buf);
                    let mut paths = parent
                        .as_deref()
                        .and_then(|d| scan_photo_folder(d).ok())
                        .unwrap_or_default();
                    if !paths.contains(&path) {
                        paths.insert(0, path.clone());
                    }
                    self.catalog.load_paths(parent, paths);
                    if let Some(pos) = self.catalog.entries.iter().position(|e| e.path == path) {
                        self.catalog.select_index(pos);
                    }
                    self.thumbnails.clear();
                    self.populate_visible_thumbnails(24);
                }

                self.image = decoded;
                self.image_generation = self.image_generation.wrapping_add(1);
                self.active_raw_decode_mode = decode_mode;
                self.recipe = restored_meta.recipe.clone();
                self.saved_recipe = restored_meta.recipe;
                self.refresh_user_profiles();
                self.clear_transient_interactions();
                self.reset_ai_select_for_photo_switch();
                self.history.clear();
                self.camera.reset();
                self.wb_eyedropper_active = false;
                self.color_slice_picker_active = false;
                self.skin_picker_active = false;
                self.camera.view_color_range = -1;
                self.cursor_readout = None;
                self.recompute_histogram();
                self.status_message = Some(format!("Loaded {name} ({dims})"));

                tx.set_represented_file(Some(path));
                tx.set_title(format!("{name} — Rune Darkroom"));
                self.sync_window_state(tx);
                self.spawn_prefetch_neighbors();
            }
            Err(err) => {
                self.status_message = Some(format!("Could not open image: {err}"));
            }
        }
    }

    pub fn open_folder_tx(&mut self, dir: PathBuf, tx: &mut Tx) {
        match scan_photo_folder(&dir) {
            Ok(paths) if !paths.is_empty() => {
                let count = paths.len();
                let folder_name = dir
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| dir.display().to_string());
                self.demo_mode = false;
                self.catalog.load_paths(Some(dir), paths);
                self.thumbnails.clear();
                if let Ok(mut cache) = self.prefetch_cache.lock() {
                    cache.clear();
                }
                self.populate_visible_thumbnails(32);
                self.show_filmstrip = true;
                self.load_active_catalog_photo_tx(tx);
                self.status_message = Some(format!("Opened folder {folder_name} ({count} photos)"));
                self.sync_native_menu();
                tx.announce(
                    format!("Opened folder {folder_name} with {count} photos"),
                    LiveRegion::Polite,
                );
            }
            Ok(_) => {
                self.status_message =
                    Some("No supported RAW or photo files found in folder".to_owned());
            }
            Err(err) => {
                self.status_message = Some(format!("Could not scan folder: {err}"));
            }
        }
    }

    /// Auto-saves dirty `.rune-edit.json` sidecar edits for `index` before switching away
    /// in a real folder session so unsaved adjustments are never silently discarded.
    fn autosave_dirty_entry_sidecar(&mut self, index: usize) {
        if self.demo_mode || !self.is_dirty() || self.image.source_path.is_none() {
            return;
        }
        let Some(entry) = self.catalog.entries.get_mut(index) else {
            return;
        };
        let diffs = rune_editor_core::recipe_field_diffs(&self.saved_recipe, &self.recipe);
        entry.meta.recipe = self.recipe.clone();
        if !diffs.is_empty() {
            entry.meta.push_history(
                rune_editor_core::EditActor::Human,
                format!("GUI auto-save ({} field(s))", diffs.len()),
                diffs,
                Some(self.saved_recipe.clone()),
            );
        }
        if entry.meta.save_sidecar_in_place(&entry.path).is_ok() {
            self.saved_recipe = self.recipe.clone();
        }
    }

    pub fn switch_catalog_index_tx(&mut self, index: usize, tx: &mut Tx) {
        if self.catalog.active_index == Some(index) {
            return;
        }
        let prev = self.catalog.active_index;
        self.catalog.update_active_recipe(self.recipe.clone());
        if self.catalog.select_index(index).is_some() {
            if let Some(prev_idx) = prev {
                self.autosave_dirty_entry_sidecar(prev_idx);
            }
            self.load_active_catalog_photo_tx(tx);
        }
    }

    pub fn select_next_photo_tx(&mut self, tx: &mut Tx) {
        let prev = self.catalog.active_index;
        self.catalog.update_active_recipe(self.recipe.clone());
        if self.catalog.select_next().is_some() {
            if let Some(prev_idx) = prev {
                self.autosave_dirty_entry_sidecar(prev_idx);
            }
            self.load_active_catalog_photo_tx(tx);
        }
    }

    pub fn select_prev_photo_tx(&mut self, tx: &mut Tx) {
        let prev = self.catalog.active_index;
        self.catalog.update_active_recipe(self.recipe.clone());
        if self.catalog.select_prev().is_some() {
            if let Some(prev_idx) = prev {
                self.autosave_dirty_entry_sidecar(prev_idx);
            }
            self.load_active_catalog_photo_tx(tx);
        }
    }

    fn load_active_catalog_photo_tx(&mut self, tx: &mut Tx) {
        let Some(entry) = self.catalog.active_entry().cloned() else {
            return;
        };
        if self.demo_mode {
            let name = entry
                .path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Alpine_Twilight_HDR.dng".to_owned());
            let mut img = (*self.image).clone();
            img.display_name = format!("{name} (Demo)");
            self.image = Arc::new(img);
            self.recipe = entry.meta.recipe.clone();
            self.saved_recipe = entry.meta.recipe;
            self.clear_transient_interactions();
            self.reset_ai_select_for_photo_switch();
            self.wb_eyedropper_active = false;
            self.color_slice_picker_active = false;
            self.skin_picker_active = false;
            self.camera.view_color_range = -1;
            self.cursor_readout = None;
            self.history.clear();
            self.recompute_histogram();
            self.status_message = Some(format!("Switched to demo frame: {name}"));
            tx.set_represented_file(None);
            tx.set_title(format!("{name} — Rune Darkroom"));
            self.sync_window_state(tx);
            return;
        }

        let path = entry.path;
        let decode_mode = Self::raw_decode_mode_for_recipe(&entry.meta.recipe);
        let use_cache = decode_mode == RawDecodeMode::Linear || !is_raw_extension(&path);
        let cached = if use_cache {
            self.prefetch_cache
                .lock()
                .ok()
                .and_then(|c| c.get(&path).cloned())
        } else {
            None
        };
        let decoded_res = match cached {
            Some(img) => Ok(img),
            None => load_file_proxy_with_mode(&path, None, decode_mode).map(Arc::new),
        };
        match decoded_res {
            Ok(decoded) => {
                let name = decoded.display_name.clone();
                let dims = format!("{}×{}", decoded.width, decoded.height);
                self.image = decoded;
                self.image_generation = self.image_generation.wrapping_add(1);
                self.active_raw_decode_mode = decode_mode;
                self.recipe = entry.meta.recipe.clone();
                self.saved_recipe = entry.meta.recipe;
                self.refresh_user_profiles();
                self.clear_transient_interactions();
                self.reset_ai_select_for_photo_switch();
                self.history.clear();
                self.camera.reset();
                self.wb_eyedropper_active = false;
                self.color_slice_picker_active = false;
                self.skin_picker_active = false;
                self.camera.view_color_range = -1;
                self.cursor_readout = None;
                self.recompute_histogram();
                self.status_message = Some(format!("Loaded {name} ({dims})"));
                tx.set_represented_file(Some(path));
                tx.set_title(format!("{name} — Rune Darkroom"));
                self.sync_window_state(tx);
                self.spawn_prefetch_neighbors();
            }
            Err(err) => {
                self.status_message = Some(format!("Failed to load photo: {err}"));
            }
        }
    }

    pub fn set_filter_tx(&mut self, filter: CullFilter, tx: &mut Tx) {
        self.catalog.update_active_recipe(self.recipe.clone());
        let prev = self.catalog.active_index;
        self.catalog.set_filter(filter);
        if self.catalog.active_index != prev && self.catalog.active_index.is_some() {
            if let Some(prev_idx) = prev {
                self.autosave_dirty_entry_sidecar(prev_idx);
            }
            self.load_active_catalog_photo_tx(tx);
        }
    }

    pub fn set_color_tag_filter_tx(&mut self, tag: Option<ColorTag>, tx: &mut Tx) {
        self.catalog.update_active_recipe(self.recipe.clone());
        let prev = self.catalog.active_index;
        self.catalog.set_color_tag_filter(tag);
        if self.catalog.active_index != prev && self.catalog.active_index.is_some() {
            if let Some(prev_idx) = prev {
                self.autosave_dirty_entry_sidecar(prev_idx);
            }
            self.load_active_catalog_photo_tx(tx);
        }
    }

    pub fn set_browser_search_tx(&mut self, query: String, tx: &mut Tx) {
        self.catalog.update_active_recipe(self.recipe.clone());
        let prev = self.catalog.active_index;
        self.catalog.set_search_query(query);
        if self.catalog.active_index != prev && self.catalog.active_index.is_some() {
            if let Some(prev_idx) = prev {
                self.autosave_dirty_entry_sidecar(prev_idx);
            }
            self.load_active_catalog_photo_tx(tx);
        }
    }

    fn advance_if_auto_tx(&mut self, tx: &mut Tx) {
        if self.auto_advance {
            self.select_next_photo_tx(tx);
        }
    }

    pub fn set_entry_color_tag(&mut self, index: usize, tag: ColorTag) -> bool {
        if self.catalog.set_color_tag(index, tag) {
            if !self.demo_mode {
                let _ = self.catalog.save_entry_sidecar_mut(index);
            }
            if self.catalog.active_index == Some(index) {
                self.sync_mcp_canvas_snapshot();
            }
            let msg = if tag == ColorTag::None {
                "Cleared color tag".to_owned()
            } else {
                format!("Tagged {}", tag.label())
            };
            self.status_message = Some(msg);
            true
        } else {
            false
        }
    }

    pub fn set_entry_color_tag_tx(&mut self, index: usize, tag: ColorTag, tx: &mut Tx) {
        let was_active = self.catalog.active_index == Some(index);
        if self.set_entry_color_tag(index, tag) {
            self.sync_window_state(tx);
            if was_active {
                self.advance_if_auto_tx(tx);
            }
        }
    }

    pub fn set_active_color_tag_tx(&mut self, tag: ColorTag, tx: &mut Tx) {
        if let Some(idx) = self.catalog.active_index {
            self.set_entry_color_tag_tx(idx, tag, tx);
        }
    }

    pub fn set_entry_rating_tx(&mut self, index: usize, rating: u8, tx: &mut Tx) {
        let was_active = self.catalog.active_index == Some(index);
        if self.catalog.set_rating(index, rating) {
            if !self.demo_mode {
                let _ = self.catalog.save_entry_sidecar_mut(index);
            }
            let stars = if rating == 0 {
                "Cleared rating".to_owned()
            } else {
                format!("Rated {rating}★")
            };
            self.status_message = Some(stars.clone());
            self.sync_window_state(tx);
            tx.announce(stars, LiveRegion::Polite);
            if was_active {
                self.advance_if_auto_tx(tx);
            }
        }
    }

    pub fn set_active_rating_tx(&mut self, rating: u8, tx: &mut Tx) {
        if let Some(idx) = self.catalog.active_index {
            self.set_entry_rating_tx(idx, rating, tx);
        }
    }

    pub fn toggle_active_flag_tx(&mut self, flag: CullFlag, tx: &mut Tx) {
        if let Some(next_flag) = self.catalog.toggle_active_flag(flag) {
            if !self.demo_mode
                && let Some(idx) = self.catalog.active_index
            {
                let _ = self.catalog.save_entry_sidecar_mut(idx);
            }
            let msg = match next_flag {
                CullFlag::Pick => "Flagged as Pick",
                CullFlag::Reject => "Flagged as Reject",
                CullFlag::None => "Cleared culling flag",
            };
            self.status_message = Some(msg.to_owned());
            self.sync_window_state(tx);
            tx.announce(msg, LiveRegion::Polite);
            self.advance_if_auto_tx(tx);
        }
    }

    pub fn export_to_path_tx(&mut self, dest: PathBuf, tx: &mut Tx) {
        if is_raw_extension(&dest) {
            self.status_message = Some(format!(
                "Export blocked: refusing to write to RAW/camera file '{}'",
                dest.display()
            ));
            return;
        }
        if let Some(src) = self.image.source_path.as_deref()
            && let Err(err) = guard_not_source(src, &dest)
        {
            self.status_message = Some(format!("Export blocked: {err}"));
            return;
        }
        let ext = dest
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let (fmt, label) = match ext.as_str() {
            "png" => (ExportFormat::Png16, "16-bit PNG"),
            "tif" | "tiff" => (ExportFormat::Tiff16, "16-bit TIFF"),
            "webp" => (ExportFormat::WebP, "WebP"),
            _ => (ExportFormat::Jpeg { quality: 92 }, "JPEG (92%)"),
        };
        let options = ExportOptions {
            format: fmt,
            respect_crop: true,
            ..ExportOptions::default()
        };
        match export_image_with_options(&self.image, &self.recipe, &dest, &options) {
            Ok(()) => {
                let name = dest
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| dest.display().to_string());
                let msg = format!("Exported {name} ({label})");
                self.status_message = Some(msg.clone());
                tx.announce(msg, LiveRegion::Polite);
            }
            Err(err) => {
                self.status_message = Some(format!("Export failed: {err}"));
            }
        }
    }

    /// Generates thumbnails for up to `limit` catalog entries that don't have one cached yet.
    fn populate_visible_thumbnails(&mut self, limit: usize) {
        let mut loaded = 0usize;
        for entry in &self.catalog.entries {
            if loaded >= limit {
                break;
            }
            if self.thumbnails.contains_key(&entry.path) {
                continue;
            }
            if let Ok(thumb) = load_thumbnail(&entry.path, 220) {
                self.thumbnails
                    .insert(entry.path.clone(), thumbnail_to_pixels(thumb));
                loaded += 1;
            }
        }
    }

    /// Prefetches adjacent catalog images on a background thread for instant `←`/`→` switching,
    /// bounding cached full-resolution images to `MAX_PREFETCH_CACHE_ENTRIES` and deduplicating
    /// in-flight decodes across rapid key repeats.
    fn spawn_prefetch_neighbors(&self) {
        if self.demo_mode {
            return;
        }
        let neighbors = self.catalog.neighbor_paths();
        if neighbors.is_empty() {
            return;
        }
        let mut to_fetch = Vec::new();
        for path in &neighbors {
            let already_cached = self
                .prefetch_cache
                .lock()
                .map(|c| c.contains_key(path))
                .unwrap_or(true);
            if already_cached {
                continue;
            }
            let claimed = self
                .prefetch_in_flight
                .lock()
                .map(|mut set| set.insert(path.clone()))
                .unwrap_or(false);
            if claimed {
                to_fetch.push(path.clone());
            }
        }
        if to_fetch.is_empty() {
            return;
        }

        let cache = Arc::clone(&self.prefetch_cache);
        let in_flight = Arc::clone(&self.prefetch_in_flight);
        std::thread::spawn(move || {
            for path in to_fetch {
                let decoded = load_file(&path);
                if let Ok(img) = decoded
                    && let Ok(mut guard) = cache.lock()
                {
                    insert_prefetch_bounded(&mut guard, path.clone(), Arc::new(img), &neighbors);
                }
                if let Ok(mut set) = in_flight.lock() {
                    set.remove(&path);
                }
            }
        });
    }
}

/// Inserts `img` into `cache`, evicting stale non-neighbor entries so at most
/// `MAX_PREFETCH_CACHE_ENTRIES` full-resolution linear buffers stay resident.
pub(crate) fn insert_prefetch_bounded(
    cache: &mut HashMap<PathBuf, Arc<LinearImage>>,
    path: PathBuf,
    img: Arc<LinearImage>,
    keep_paths: &[PathBuf],
) {
    if !cache.contains_key(&path) {
        while cache.len() >= MAX_PREFETCH_CACHE_ENTRIES {
            let victim = cache
                .keys()
                .find(|k| !keep_paths.contains(*k))
                .or_else(|| cache.keys().next())
                .cloned();
            if let Some(k) = victim {
                cache.remove(&k);
            } else {
                break;
            }
        }
    }
    cache.insert(path, img);
}

fn thumbnail_to_pixels(thumb: ThumbnailRgba8) -> Pixels {
    let px = Pixels::new();
    px.set(thumb.width, thumb.height, thumb.rgba);
    px
}

/// Renders a graded sRGB RGBA8 thumbnail for a demo filmstrip frame so every card
/// shows a genuine photographic preview of its look.
fn render_graded_demo_pixels(img: &LinearImage, recipe: &DevelopRecipe, max_edge: u32) -> Pixels {
    if recipe.is_neutral() {
        return thumbnail_to_pixels(generate_demo_thumbnail(img, max_edge));
    }
    let aspect = img.width as f32 / img.height.max(1) as f32;
    let (dw, dh) = if aspect >= 1.0 {
        (max_edge, ((max_edge as f32 / aspect).round() as u32).max(1))
    } else {
        (((max_edge as f32 * aspect).round() as u32).max(1), max_edge)
    };
    let compiled = CompiledRecipe::from_recipe(recipe);
    let sw = img.width as usize;
    let sh = img.height as usize;
    let mut rgba = vec![0u8; (dw as usize) * (dh as usize) * 4];
    for dy in 0..dh as usize {
        let sy = (dy * sh / dh as usize).min(sh - 1);
        for dx in 0..dw as usize {
            let sx = (dx * sw / dw as usize).min(sw - 1);
            let s_idx = (sy * sw + sx) * 4;
            let lin = [
                img.rgba16f[s_idx].to_f32(),
                img.rgba16f[s_idx + 1].to_f32(),
                img.rgba16f[s_idx + 2].to_f32(),
            ];
            let graded = compiled.evaluate_pixel_linear(lin);
            let d_idx = (dy * dw as usize + dx) * 4;
            rgba[d_idx] = (linear_to_srgb(graded[0]) * 255.0).round() as u8;
            rgba[d_idx + 1] = (linear_to_srgb(graded[1]) * 255.0).round() as u8;
            rgba[d_idx + 2] = (linear_to_srgb(graded[2]) * 255.0).round() as u8;
            rgba[d_idx + 3] = 255;
        }
    }
    let px = Pixels::new();
    px.set(dw, dh, rgba);
    px
}

fn build_demo_session(
    image: &LinearImage,
    presets: &[StylePreset],
) -> (SessionCatalog, HashMap<PathBuf, Pixels>) {
    let find_recipe = |id: &str| {
        presets
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.recipe.clone())
            .unwrap_or_default()
    };

    let frames: [(&str, &str, u8, CullFlag, ColorTag, &str, &str); 5] = [
        (
            "Editorial_Portrait.jpg",
            "neutral",
            5,
            CullFlag::Pick,
            ColorTag::Green,
            "Leica Q3",
            "Summilux 28mm f/1.7",
        ),
        (
            "Morning_Coffee_Warm.jpg",
            "warm_film",
            4,
            CullFlag::Pick,
            ColorTag::Orange,
            "Leica Q3",
            "Summilux 28mm f/1.7",
        ),
        (
            "Classic_Chrome_Film.jpg",
            "classic_chrome",
            4,
            CullFlag::None,
            ColorTag::Blue,
            "Fujifilm GFX100 II",
            "GF 55mm f/1.7",
        ),
        (
            "Editorial_Mono.jpg",
            "editorial_bw",
            3,
            CullFlag::None,
            ColorTag::Purple,
            "Leica M11 Monochrom",
            "APO-Summicron 50mm",
        ),
        (
            "Soft_Dusk_Tone.jpg",
            "moody_dusk",
            2,
            CullFlag::Reject,
            ColorTag::Red,
            "Sony A7R V",
            "FE 35mm f/1.4 GM",
        ),
    ];

    let mut entries = Vec::with_capacity(frames.len());
    let mut thumbs = HashMap::with_capacity(frames.len());
    for (name, preset_id, rating, flag, color_tag, cam, lens) in frames {
        let path = PathBuf::from(format!("demo/{name}"));
        let recipe = find_recipe(preset_id);
        let px = render_graded_demo_pixels(image, &recipe, 240);
        thumbs.insert(path.clone(), px);
        let meta = PhotoMeta {
            rating,
            flag,
            color_tag,
            recipe,
            ..PhotoMeta::default()
        };
        let mut entry = SessionEntry::new(path, meta);
        entry.camera_label = Some(cam.to_owned());
        entry.lens_label = Some(lens.to_owned());
        entries.push(entry);
    }

    let catalog = SessionCatalog {
        folder: None,
        entries,
        active_index: Some(0),
        filter: CullFilter::All,
        color_tag_filter: None,
        search_query: String::new(),
        clipboard: None,
    };

    (catalog, thumbs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_edit_keys_do_not_conflict_with_ratings_or_panels() {
        for reserved in ["0", "1", "2", "3", "4", "5", "p", "x", "f", "i", "c", "\\"] {
            assert_eq!(SpeedEditParam::from_key(reserved), None);
        }
        assert_eq!(
            SpeedEditParam::from_key("q"),
            Some(SpeedEditParam::Exposure)
        );
        assert_eq!(
            SpeedEditParam::from_key("Q"),
            Some(SpeedEditParam::Exposure)
        );
        assert_eq!(
            SpeedEditParam::from_key("t"),
            Some(SpeedEditParam::Temperature)
        );
        assert_eq!(SpeedEditParam::from_key("y"), Some(SpeedEditParam::Tint));
    }

    #[test]
    fn speed_edit_scrub_clamps_and_produces_single_undo_snapshot() {
        let mut s = AppState::default();
        s.begin_speed_edit(SpeedEditParam::Exposure);
        assert!(s.scrub_speed_edit(0.25));
        assert!(s.scrub_speed_edit(0.25));
        assert!(s.recipe.exposure > 1.0);
        // Repeated scrubs before release must keep ONE baseline snapshot and NOT flood history.
        assert!(!s.history.can_undo());
        assert!(s.speed_edit_snapshot.is_some());

        // Extreme scrub clamps safely to registry +/-5.0 EV without NaN/Inf.
        s.scrub_speed_edit(100.0);
        assert_eq!(s.recipe.exposure, 5.0);
        s.scrub_speed_edit(-100.0);
        assert_eq!(s.recipe.exposure, -5.0);
        assert!(!s.scrub_speed_edit(f32::NAN));
    }

    #[test]
    fn apply_preset_preserves_geometry() {
        let mut s = AppState::default();
        s.recipe.geometry.crop = [0.1, 0.15, 0.75, 0.7];
        s.recipe.geometry.straighten_deg = 3.25;
        s.recipe.geometry.rotate_90 = 1;
        s.recipe.geometry.flip_h = true;
        s.recipe.geometry.aspect_preset = CropAspect::FourFive;
        let expected_geo = s.recipe.geometry;

        s.apply_preset("warm_film");

        assert_eq!(s.recipe.geometry, expected_geo);
        assert_eq!(s.recipe.preset_name.as_deref(), Some("Warm Film"));
        assert!(s.recipe.temperature > 0.0);
    }

    #[test]
    fn exposure_bounds_match_core_registry() {
        let spec = rune_editor_core::find_scalar_param("exposure")
            .expect("exposure must be registered in SCALAR_PARAMS");
        assert_eq!(exposure_bounds(), (spec.min, spec.max));
        assert_eq!(exposure_bounds(), (-5.0, 5.0));
    }

    #[test]
    fn prefetch_cache_is_bounded_to_two_entries() {
        let mut cache: HashMap<PathBuf, Arc<LinearImage>> = HashMap::new();
        let img = Arc::new(generate_demo_hdr_scene(16, 16));
        let keep = vec![PathBuf::from("d.arw"), PathBuf::from("e.arw")];
        for name in ["a.arw", "b.arw", "c.arw", "d.arw", "e.arw"] {
            insert_prefetch_bounded(&mut cache, PathBuf::from(name), Arc::clone(&img), &keep);
            assert!(cache.len() <= MAX_PREFETCH_CACHE_ENTRIES);
        }
        assert_eq!(cache.len(), 2);
        assert!(cache.contains_key(&PathBuf::from("d.arw")));
        assert!(cache.contains_key(&PathBuf::from("e.arw")));
    }

    #[test]
    fn cursor_readout_samples_rgb_hsl_lab_without_touching_history() {
        let mut s = AppState {
            viewport_rect_pts: [800.0, 600.0],
            viewport_scale: 2.0,
            ..AppState::default()
        };
        let hist_before = s.histogram.clone();
        s.update_cursor_readout(400.0, 300.0);
        let probe = s.cursor_readout.expect("center pixel should sample");
        assert!(probe.hsl[0].is_finite() && probe.lab[0].is_finite());
        assert_eq!(s.histogram.luma, hist_before.luma);
        // Outside photo frame clears readout cleanly
        s.update_cursor_readout(2.0, 2.0);
        assert!(s.cursor_readout.is_none());
    }

    #[test]
    fn crop_drag_respects_bounds_and_aspect_ratio() {
        let mut s = AppState {
            viewport_rect_pts: [800.0, 600.0],
            viewport_scale: 1.0,
            ..AppState::default()
        };
        s.camera.crop_mode = true;
        s.recipe
            .geometry
            .apply_aspect_preset(CropAspect::Square, s.image.width, s.image.height);
        let before = s.recipe.geometry.crop;
        // Drag top-left corner inward
        s.crop_drag = Some((CropDragHandle::TopLeft, [before[0], before[1]], before));
        s.update_crop_drag(420.0, 320.0, 800.0, 600.0);
        let [x, y, w, h] = s.recipe.geometry.crop;
        assert!(x >= 0.0 && y >= 0.0 && x + w <= 1.0001 && y + h <= 1.0001);
        let [out_w, out_h] = s
            .recipe
            .geometry
            .cropped_output_size(s.image.width, s.image.height);
        assert!((out_w as i32 - out_h as i32).abs() <= 2);
    }

    #[test]
    fn all_gui_scalar_bounds_match_core_registry() {
        let gui_ids = [
            "profile_strength",
            "exposure",
            "contrast",
            "brightness",
            "highlights",
            "shadows",
            "whites",
            "blacks",
            "temperature",
            "tint",
            "vibrance",
            "saturation",
            "clarity",
            "structure",
            "dehaze",
            "sharpen",
            "sharpen_radius",
            "sharpen_threshold",
            "sharpen_halo",
            "noise_reduction",
            "nr_color",
            "vignette",
            "grain",
            "grain_size",
        ];
        assert_eq!(gui_ids.len(), rune_editor_core::SCALAR_PARAMS.len());
        for id in gui_ids {
            let spec = find_scalar_param(id)
                .unwrap_or_else(|| panic!("missing `{id}` in core SCALAR_PARAMS"));
            assert_eq!(scalar_bounds(id), (spec.min, spec.max));
            assert_eq!(scalar_range(id), spec.min..=spec.max);
            assert_eq!(scalar_default(id), spec.default);
        }
    }

    #[test]
    fn phase1_state_mutations_record_undo_and_sync_mcp_snapshot() {
        let mut s = AppState::default();
        s.recipe.preset_name = Some("Warm Film".to_owned());

        // Film curve
        s.set_film_curve(FilmCurve::FilmExtraShadow);
        assert_eq!(s.recipe.film_curve, FilmCurve::FilmExtraShadow);
        assert!(s.recipe.preset_name.is_none());
        assert!(s.history.can_undo());
        assert_eq!(
            s.mcp_shared_canvas.lock().unwrap().recipe.film_curve,
            FilmCurve::FilmExtraShadow
        );

        // WB Preset
        s.apply_wb_preset(WbPreset::Cloudy);
        assert_eq!(s.recipe.wb_preset, WbPreset::Cloudy);
        assert!(temperature_to_kelvin(s.recipe.temperature) >= 6400);
        assert_eq!(
            s.mcp_shared_canvas.lock().unwrap().recipe.wb_preset,
            WbPreset::Cloudy
        );

        // Clarity method
        s.set_clarity_method(ClarityMethod::Punch);
        assert_eq!(s.recipe.clarity_method, ClarityMethod::Punch);
        assert_eq!(
            s.mcp_shared_canvas.lock().unwrap().recipe.clarity_method,
            ClarityMethod::Punch
        );

        // Grain type
        s.set_grain_type(GrainType::SilverRich);
        assert_eq!(s.recipe.grain_type, GrainType::SilverRich);
        assert_eq!(
            s.mcp_shared_canvas.lock().unwrap().recipe.grain_type,
            GrainType::SilverRich
        );

        // Undo unwinds each change cleanly and updates MCP snapshot
        s.undo();
        assert_eq!(s.recipe.grain_type, GrainType::Fine);
        assert_eq!(
            s.mcp_shared_canvas.lock().unwrap().recipe.grain_type,
            GrainType::Fine
        );
        s.undo();
        assert_eq!(s.recipe.clarity_method, ClarityMethod::Natural);
        s.undo();
        assert_eq!(s.recipe.wb_preset, WbPreset::AsShot);
        s.undo();
        assert_eq!(s.recipe.film_curve, FilmCurve::FilmStandard);
    }

    #[test]
    fn five_tab_tone_curve_and_per_channel_levels_helpers() {
        assert_eq!(CurveChannel::ALL.len(), 5);
        assert_eq!(CurveChannel::ALL[0], CurveChannel::Rgb);

        let mut curves = ToneCurves::default();
        CurveChannel::Rgb
            .points_mut(&mut curves)
            .insert(1, [0.5, 0.62]);
        assert_eq!(curves.rgb.len(), 3);
        assert_eq!(CurveChannel::Rgb.points(&curves).len(), 3);

        let probe = CursorColorReadout {
            rgb8: [200, 120, 40],
            hsl: [30.0, 67.0, 47.0],
            lab: [58.0, 24.0, 52.0],
        };
        for ch in CurveChannel::ALL {
            let t = ch.probe_tone(&probe);
            assert!((0.0..=1.0).contains(&t));
        }
        assert!(CurveChannel::Red.probe_tone(&probe) > CurveChannel::Blue.probe_tone(&probe));

        let mut lv = LevelsParams::default();
        assert!(!LevelsChannel::Red.is_modified(&lv));
        LevelsChannel::Red.set_in_black(&mut lv, 0.12);
        LevelsChannel::Red.set_in_mid(&mut lv, 0.25);
        LevelsChannel::Red.set_in_white(&mut lv, 0.90);
        assert!(LevelsChannel::Red.is_modified(&lv));
        assert!(!LevelsChannel::Green.is_modified(&lv));
        assert!(!LevelsChannel::Rgb.is_modified(&lv));
        lv.red = ChannelLevels::default();
        assert!(!LevelsChannel::Red.is_modified(&lv));
    }

    #[test]
    fn phase2_descriptor_bounds_match_core_registry() {
        let phase2_ids = [
            "advanced_color.0.hue_range",
            "advanced_color.0.sat_range",
            "advanced_color.0.smoothness",
            "advanced_color.0.uniformity",
            "advanced_color.0.hue_shift",
            "advanced_color.0.saturation",
            "advanced_color.0.lightness",
            "skin_tone.smoothness",
            "skin_tone.uniformity_hue",
            "skin_tone.uniformity_sat",
            "skin_tone.uniformity_lightness",
            "skin_tone.amount_hue",
            "skin_tone.amount_sat",
            "skin_tone.amount_lightness",
            "lens.distortion",
            "lens.chromatic_aberration",
            "lens.purple_fringing",
            "lens.light_falloff",
            "geometry.keystone_vertical",
            "geometry.keystone_horizontal",
            "geometry.keystone_aspect",
            "geometry.keystone_amount",
        ];
        let descriptors = all_param_descriptors();
        for id in phase2_ids {
            let desc = descriptors
                .iter()
                .find(|d| d.id == id)
                .unwrap_or_else(|| panic!("missing `{id}` in core param descriptors"));
            assert_eq!(
                descriptor_bounds(id),
                (desc.min.unwrap(), desc.max.unwrap())
            );
            assert_eq!(descriptor_range(id), desc.min.unwrap()..=desc.max.unwrap());
            let expected_def = desc
                .default
                .as_ref()
                .and_then(|v| v.as_f64())
                .expect("descriptor default must be numeric") as f32;
            assert!((descriptor_default(id) - expected_def).abs() < 1e-5);
        }
        assert!((descriptor_default("geometry.keystone_amount") - 0.8).abs() < 1e-5);
    }

    #[test]
    fn phase2_pickers_vignette_method_undo_and_mcp_sync() {
        let mut s = AppState {
            viewport_rect_pts: [800.0, 600.0],
            viewport_scale: 1.0,
            ..AppState::default()
        };
        s.recipe.preset_name = Some("Warm Film".to_owned());

        // Vignette method records undo & syncs MCP snapshot
        s.set_vignette_method(VignetteMethod::CircularOnCrop);
        assert_eq!(s.recipe.vignette_method, VignetteMethod::CircularOnCrop);
        assert!(s.recipe.preset_name.is_none());
        assert!(s.history.can_undo());
        assert_eq!(
            s.mcp_shared_canvas.lock().unwrap().recipe.vignette_method,
            VignetteMethod::CircularOnCrop
        );

        // Pick Advanced Color Slice from photo center
        s.selected_color_slice = 1;
        s.color_slice_picker_active = true;
        assert!(s.pick_color_slice_at_viewport(400.0, 300.0));
        assert!(!s.color_slice_picker_active);
        assert!(s.recipe.advanced_color[1].enabled);
        assert!(s.mcp_shared_canvas.lock().unwrap().recipe.advanced_color[1].enabled);

        // Pick Skin Tone from photo center
        s.skin_picker_active = true;
        assert!(s.pick_skin_tone_at_viewport(400.0, 300.0));
        assert!(!s.skin_picker_active);
        assert!(s.recipe.skin_tone.enabled);
        assert!(s.mcp_shared_canvas.lock().unwrap().recipe.skin_tone.enabled);

        // View Selected Color Range tracks slice switches and clears on mode/tab change
        s.set_active_tab(InspectorTab::Color);
        s.set_color_editor_mode(ColorEditorMode::Advanced);
        s.toggle_view_color_range(1);
        assert_eq!(s.camera.view_color_range, 1);
        s.set_selected_color_slice(2);
        assert_eq!(s.camera.view_color_range, 2);
        s.set_color_editor_mode(ColorEditorMode::Basic);
        assert_eq!(s.camera.view_color_range, -1);

        s.set_color_editor_mode(ColorEditorMode::SkinTone);
        s.toggle_view_color_range(4);
        assert_eq!(s.camera.view_color_range, 4);
        s.set_active_tab(InspectorTab::Light);
        assert_eq!(s.camera.view_color_range, -1);

        // Undo unwinds Skin Tone -> Color Slice -> Vignette Method cleanly
        s.undo();
        assert!(!s.recipe.skin_tone.enabled);
        s.undo();
        assert!(!s.recipe.advanced_color[1].enabled);
        s.undo();
        assert_eq!(s.recipe.vignette_method, VignetteMethod::EllipticOnCrop);
        assert_eq!(
            s.mcp_shared_canvas.lock().unwrap().recipe.vignette_method,
            VignetteMethod::EllipticOnCrop
        );

        // Eyedroppers respect Stage 1..=8 WB/Exposure grade and lens distortion UV warp
        s.recipe.exposure = 1.25;
        s.recipe.temperature = -0.45;
        s.recompute_histogram();
        assert!(s.pick_skin_tone_at_viewport(400.0, 300.0));
        let shifted_hue = s.recipe.skin_tone.target_hue;
        let shifted_lum = s.recipe.skin_tone.target_lightness;

        s.recipe.exposure = 0.0;
        s.recipe.temperature = 0.0;
        s.recompute_histogram();
        assert!(s.pick_skin_tone_at_viewport(400.0, 300.0));
        assert!(
            (shifted_hue - s.recipe.skin_tone.target_hue).abs() > 1.0
                || (shifted_lum - s.recipe.skin_tone.target_lightness).abs() > 0.05
        );

        // Lens distortion shifts off-center viewport UV mapping
        let uv_flat = s.viewport_local_to_source_uv(280.0, 180.0).unwrap();
        s.recipe.lens.distortion = -0.45;
        let uv_distorted = s.viewport_local_to_source_uv(280.0, 180.0).unwrap();
        assert!(
            (uv_flat[0] - uv_distorted[0]).abs() > 1e-3
                || (uv_flat[1] - uv_distorted[1]).abs() > 1e-3
        );
    }

    #[test]
    fn phase3_color_tags_variants_compare_and_auto_advance() {
        let mut s = AppState::default();

        // Color tag assignment + faceted filtering + live MCP snapshot sync
        assert!(s.set_entry_color_tag(0, ColorTag::Pink));
        assert_eq!(s.catalog.entries[0].effective_color_tag(), ColorTag::Pink);
        assert_eq!(
            s.mcp_shared_canvas.lock().unwrap().color_tag,
            ColorTag::Pink
        );
        s.catalog.set_color_tag_filter(Some(ColorTag::Pink));
        assert_eq!(s.catalog.filtered_indices(), vec![0]);
        s.catalog.set_color_tag_filter(None);

        // Virtual variants: new, clone, select, delete keep saved_recipe & color_tag synced
        s.recipe.exposure = 0.75;
        let v2 = s.clone_variant(Some("Warm Study".to_owned()));
        assert_eq!(v2, 1);
        assert_eq!(s.active_variant_count(), 2);
        assert!((s.recipe.exposure - 0.75).abs() < 1e-5);
        assert!(!s.is_dirty());
        assert_eq!(s.catalog.entries[0].effective_color_tag(), ColorTag::Pink);
        s.recipe.exposure = -0.5;
        assert!(s.is_dirty());
        let v3 = s.new_variant(Some("Fresh Neutral".to_owned()));
        assert_eq!(v3, 2);
        assert_eq!(s.recipe.exposure, 0.0);
        assert!(!s.is_dirty());
        assert_eq!(s.catalog.entries[0].effective_color_tag(), ColorTag::None);
        assert_eq!(
            s.mcp_shared_canvas.lock().unwrap().color_tag,
            ColorTag::None
        );
        assert!(s.select_variant(1));
        assert!((s.recipe.exposure - (-0.5)).abs() < 1e-5);
        assert!(!s.is_dirty());
        assert_eq!(s.catalog.entries[0].effective_color_tag(), ColorTag::Pink);
        assert_eq!(
            s.mcp_shared_canvas.lock().unwrap().color_tag,
            ColorTag::Pink
        );

        // Compare Variant pins reference frame
        s.pin_compare_variant();
        let cmp = s
            .compare_reference
            .as_ref()
            .expect("compare reference pinned");
        assert_eq!(cmp.variant_tag, "V2");
        assert!(s.delete_active_variant());
        assert_eq!(s.active_variant_count(), 2);
    }

    #[test]
    fn phase3_selective_clipboard_stacked_styles_loupe_and_readouts() {
        let mut s = AppState {
            viewport_rect_pts: [800.0, 600.0],
            viewport_scale: 1.0,
            ..AppState::default()
        };

        // Selective Adjustments Clipboard + Autoselect Adjusted
        s.recipe.temperature = 0.25;
        s.recipe.exposure = 0.65;
        s.recipe.clarity = 0.30;
        s.autoselect_adjusted_groups();
        assert!(s.clipboard_groups.contains("wb"));
        assert!(s.clipboard_groups.contains("tone"));
        assert!(s.clipboard_groups.contains("detail"));

        s.catalog.copy_adjustments(&s.recipe);
        s.recipe = DevelopRecipe::default();
        s.clipboard_groups.clear();
        s.clipboard_groups.insert("wb".to_owned());
        assert!(s.paste_selected_adjustments());
        assert!((s.recipe.temperature - 0.25).abs() < 1e-5);
        assert_eq!(s.recipe.exposure, 0.0);
        assert!(s.history.can_undo());

        // Stacked Style preserves geometry and blends additively
        s.recipe.geometry.crop = [0.1, 0.1, 0.8, 0.8];
        s.stack_styles = true;
        s.style_strength = 0.5;
        s.apply_preset("warm_film");
        assert_eq!(s.recipe.geometry.crop, [0.1, 0.1, 0.8, 0.8]);

        // Loupe camera UV & Pinned Color Readouts (#1..#4)
        s.camera.loupe_enabled = true;
        s.update_loupe_uv(600.0, 300.0);
        assert!((s.camera.loupe_uv[0] - 0.75).abs() < 1e-5);
        assert!((s.camera.loupe_uv[1] - 0.5).abs() < 1e-5);

        for i in 0..5 {
            assert!(s.pin_color_readout_at_viewport(360.0 + i as f32 * 10.0, 300.0));
        }
        assert_eq!(s.pinned_readouts.len(), MAX_PINNED_READOUTS);
        let live = s.evaluate_pinned_readout(&s.pinned_readouts[0]);
        assert!(live.lab[0].is_finite());

        // Naming template preview expansion
        let spec = &s.export_recipes[0];
        let preview = s.preview_export_filename(spec);
        assert!(preview.ends_with(".jpg"));
        assert!(preview.contains("Editorial_Portrait"));
    }

    #[test]
    fn phase4_layers_masks_ai_subject_heal_undo_and_mcp_sync() {
        let mut s = AppState {
            viewport_rect_pts: [800.0, 600.0],
            viewport_scale: 1.0,
            ..AppState::default()
        };

        // Add Linear Gradient layer -> updates active_layer_index, camera, history & MCP snapshot
        let l0 = s
            .add_layer_with_shape(
                "Sky Grade".to_owned(),
                Some(MaskShape::LinearGradient {
                    start: [0.5, 0.1],
                    end: [0.5, 0.6],
                    feather: 0.7,
                }),
            )
            .expect("layer 0 added");
        assert_eq!(l0, 0);
        assert_eq!(s.active_layer_index, Some(0));
        assert_eq!(s.camera.active_layer_index, 0);
        assert!(s.history.can_undo());
        assert_eq!(s.mcp_shared_canvas.lock().unwrap().recipe.layers.len(), 1);

        // Combine Masks: add Radial mask and switch combine op to Subtract
        assert!(s.add_mask_to_active_layer(MaskShape::RadialGradient {
            center: [0.5, 0.45],
            radius: [0.25, 0.30],
            angle_deg: 0.0,
            feather: 0.4,
        }));
        assert_eq!(s.recipe.layers[0].masks.len(), 2);
        assert!(s.set_layer_mask_op(0, 1, MaskCombineOp::Subtract));
        assert_eq!(s.recipe.layers[0].masks[1].op, MaskCombineOp::Subtract);

        // Mask overlay shortcuts (M -> Red, ⌥M -> Grayscale, ⌥M -> Off) & Invert
        s.toggle_red_mask_overlay();
        assert_eq!(s.camera.mask_overlay_mode, 1);
        s.toggle_gray_mask_overlay();
        assert_eq!(s.camera.mask_overlay_mode, 2);
        s.toggle_gray_mask_overlay();
        assert_eq!(s.camera.mask_overlay_mode, 0);
        assert!(s.toggle_active_layer_invert());
        assert!(s.recipe.layers[0].inverted);

        // AI Subject mask generation adds a valid 512×512 RasterMask layer
        const MASK_DIM: u32 = rune_editor_core::LAYER_MASK_TILE_SIZE as u32;
        let l1 = s
            .add_ai_subject_layer(false)
            .expect("AI subject layer added");
        assert_eq!(l1, 1);
        assert!(matches!(
            s.recipe.layers[1].masks[0].shape,
            MaskShape::RasterMask {
                width: MASK_DIM,
                height: MASK_DIM,
                ..
            }
        ));
        assert!(s.last_ai_mask_status.is_some());
        assert_eq!(s.mcp_shared_canvas.lock().unwrap().recipe.layers.len(), 2);

        // Spot Heal & Clone creation + MCP sync + undo
        let h0 = s
            .add_heal_spot([0.45, 0.35], None, HealMode::Heal)
            .expect("heal spot added");
        assert_eq!(h0, 0);
        assert_eq!(
            s.mcp_shared_canvas.lock().unwrap().recipe.heal_spots.len(),
            1
        );
        s.undo();
        assert!(s.recipe.heal_spots.is_empty());
        assert!(
            s.mcp_shared_canvas
                .lock()
                .unwrap()
                .recipe
                .heal_spots
                .is_empty()
        );

        // Layer deletion keeps active_layer_index & camera valid
        assert!(s.remove_layer(1));
        assert_eq!(s.active_layer_index, Some(0));
        assert_eq!(s.camera.active_layer_index, 0);
        assert!(s.remove_layer(0));
        assert_eq!(s.active_layer_index, None);
        assert_eq!(s.camera.active_layer_index, -1);

        // Vector guide projection roundtrip parity under combined rotation (incl. negative turns) + flip
        for &(rot, fh, fv) in &[
            (1i8, true, false),
            (3i8, false, true),
            (-1i8, true, false),
            (2i8, true, true),
        ] {
            s.recipe.geometry.rotate_90 = rot;
            s.recipe.geometry.flip_h = fh;
            s.recipe.geometry.flip_v = fv;
            let [su, sv] = s.recipe.geometry.oriented_to_source_uv_unclamped(
                0.28,
                0.37,
                s.image.width,
                s.image.height,
            );
            let [lx, ly] = s.source_uv_to_viewport_local([su, sv], 800.0, 600.0);
            let [rt_su, rt_sv] = s
                .viewport_local_to_source_uv(lx, ly)
                .expect("projected guide point must map back inside photo");
            assert!(
                (rt_su - su).abs() < 1e-3 && (rt_sv - sv).abs() < 1e-3,
                "roundtrip mismatch for rot={rot}, fh={fh}, fv={fv}: got ({rt_su},{rt_sv}) vs ({su},{sv})"
            );
        }

        // Brush dab cap is enforced in begin_canvas_tool_drag as well as update_canvas_tool_drag
        s.recipe.geometry = rune_editor_core::GeometryParams::default();
        s.add_layer_with_shape(
            "Brush Cap".to_owned(),
            Some(MaskShape::BrushStrokes {
                strokes: vec![BrushDab::default(); MAX_BRUSH_DABS_PER_MASK],
            }),
        );
        s.set_canvas_tool(CanvasTool::Brush);
        s.begin_canvas_tool_drag(400.0, 300.0);
        if let MaskShape::BrushStrokes { strokes } = &s.recipe.layers[0].masks[0].shape {
            assert_eq!(strokes.len(), MAX_BRUSH_DABS_PER_MASK);
        } else {
            panic!("expected BrushStrokes mask");
        }
    }

    #[test]
    fn phase5_film_negative_proof_margin_hot_folder_and_shortcuts() {
        let mut s = AppState {
            viewport_rect_pts: [800.0, 600.0],
            viewport_scale: 1.0,
            ..AppState::default()
        };

        // Film Negative mode + base picker + undo + MCP snapshot sync
        s.set_processing_mode(ProcessingMode::FilmNegative);
        assert_eq!(s.recipe.processing_mode, ProcessingMode::FilmNegative);
        assert_eq!(
            s.mcp_shared_canvas.lock().unwrap().recipe.processing_mode,
            ProcessingMode::FilmNegative
        );
        assert!(s.history.can_undo());

        s.film_base_picker_active = true;
        assert!(s.pick_film_base_at_viewport(400.0, 300.0));
        assert!(!s.film_base_picker_active);
        assert_eq!(s.recipe.processing_mode, ProcessingMode::FilmNegative);
        assert!(s.recipe.film_negative.base_r >= 0.05);
        s.undo();
        assert_eq!(s.recipe.film_negative, FilmNegativeParams::default());

        // Proof Margin cycling (Compact -> Studio -> Exhibition -> Compact)
        s.proof_margin = ProofMargin::Compact;
        s.cycle_proof_margin();
        assert_eq!(s.proof_margin, ProofMargin::Studio);
        s.cycle_proof_margin();
        assert_eq!(s.proof_margin, ProofMargin::Exhibition);
        s.cycle_proof_margin();
        assert_eq!(s.proof_margin, ProofMargin::Compact);

        // Hot Folder watcher initialization & polling on a temp directory
        let tmp_dir =
            std::env::temp_dir().join(format!("rune_darkroom_p5_hotfolder_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp_dir);
        std::fs::create_dir_all(&tmp_dir).unwrap();
        assert!(s.start_hot_folder_watch(tmp_dir.clone()).is_ok());
        assert_eq!(s.hot_folder_dir.as_deref(), Some(tmp_dir.as_path()));
        assert_eq!(s.poll_hot_folder(), 0);
        s.stop_hot_folder_watch();
        assert!(s.hot_folder_watcher.is_none());
        let _ = std::fs::remove_dir_all(&tmp_dir);

        // Custom Shortcut persistence & backward-compatible settings.json roundtrip
        let mut persisted = PersistedAppSettings::default();
        persisted.mcp.enabled = true;
        persisted.mcp.port = 37421;
        persisted.proof_margin = ProofMargin::Exhibition;
        persisted.hot_folder_style = Some("warm_film".to_owned());
        assert!(
            persisted
                .shortcuts
                .set_key(ShortcutAction::Loupe, "z")
                .is_ok()
        );
        assert!(
            persisted
                .shortcuts
                .set_key(ShortcutAction::Brush, "3")
                .is_err()
        );

        let json = persisted.to_json_pretty();
        let reloaded = PersistedAppSettings::from_json_str(&json, persisted.mcp.clone());
        assert_eq!(reloaded.proof_margin, ProofMargin::Exhibition);
        assert_eq!(reloaded.hot_folder_style.as_deref(), Some("warm_film"));
        assert_eq!(reloaded.shortcuts.key_for(ShortcutAction::Loupe), "z");
        assert!(reloaded.shortcuts.is_customized(ShortcutAction::Loupe));

        // Legacy settings.json (McpSettings only) loads cleanly with defaults
        let legacy = r#"{"enabled": true, "port": 37420, "allow_live_canvas_edits": false}"#;
        let from_legacy = PersistedAppSettings::from_json_str(legacy, McpSettings::default());
        assert_eq!(from_legacy.proof_margin, ProofMargin::Compact);
        assert!(!from_legacy.shortcuts.is_any_customized());

        // Reset defaults clears overrides
        let mut sc = persisted.shortcuts;
        sc.reset_defaults();
        assert!(!sc.is_any_customized());
        assert_eq!(sc.key_for(ShortcutAction::Loupe), "o");
    }

    #[test]
    fn studio_airy_progressive_disclosure_hovered_info_and_overlays_popover() {
        let mut s = AppState::default();

        // 1. Calmer defaults: primary cards open, secondary cards collapsed
        assert!(s.base_open);
        assert!(s.wb_open);
        assert!(s.tone_open);
        assert!(s.color_balance_open);
        assert!(s.presence_open);
        assert!(s.clarity_open);
        assert!(s.crop_open);
        assert!(s.rotate_open);
        assert!(s.layers_open);
        assert!(s.styles_open);

        assert!(!s.levels_open);
        assert!(!s.curve_open);
        assert!(!s.hsl_open);
        assert!(!s.bw_open);
        assert!(!s.sharpen_open);
        assert!(!s.nr_open);
        assert!(!s.grain_open);
        assert!(!s.vignette_open);
        assert!(!s.keystone_open);
        assert!(!s.lens_open);
        assert!(!s.grid_open);
        assert!(!s.masks_open);
        assert!(!s.heal_open);
        assert!(!s.user_styles_open);
        assert!(!s.clipboard_open);
        assert!(!s.variants_open);

        // 2. Tier 1 inline Advanced disclosure defaults collapsed and toggles cleanly
        assert!(!s.tone_advanced_open);
        assert!(!s.levels_advanced_open);
        assert!(!s.sharpen_advanced_open);
        assert!(!s.grain_advanced_open);
        assert!(!s.vignette_advanced_open);
        assert!(!s.keystone_advanced_open);
        assert!(!s.lens_advanced_open);
        assert!(!s.local_adj_advanced_open);
        assert!(!s.film_neg_advanced_open);
        s.tone_advanced_open = true;
        assert!(s.tone_advanced_open);

        // 3. Settings progressive disclosure defaults collapsed and toggles cleanly
        assert!(!s.settings_mcp_snippets_open);
        assert!(!s.settings_mcp_tools_open);
        assert!(!s.settings_security_invariants_open);
        s.settings_mcp_snippets_open = true;
        s.settings_mcp_tools_open = true;
        s.settings_security_invariants_open = true;
        assert!(s.settings_mcp_snippets_open);
        assert!(s.settings_mcp_tools_open);
        assert!(s.settings_security_invariants_open);

        // 4. Hover `ⓘ` info tip is purely transient (never dirties recipe or pushes undo)
        let tip_a = InfoTip {
            title: "Exposure & Tone",
            shortcut: Some("Q"),
            description: "Scene-referred EV exposure.",
            cli_key: "exposure",
            tier: "Tier 0 · Tone",
        };
        let tip_b = InfoTip {
            title: "White Balance",
            shortcut: Some("T"),
            description: "Kelvin calibration.",
            cli_key: "temperature",
            tier: "Tier 0 · Light",
        };
        assert!(!s.is_dirty());
        assert!(!s.history.can_undo());
        s.set_hovered_info(tip_a);
        assert_eq!(s.hovered_info, Some(tip_a));
        assert!(!s.is_dirty());
        assert!(!s.history.can_undo());
        // Clearing a non-matching title preserves the current hovered tip
        s.clear_hovered_info(tip_b.title);
        assert_eq!(s.hovered_info, Some(tip_a));
        // Clearing the matching title clears hovered_info
        s.clear_hovered_info(tip_a.title);
        assert_eq!(s.hovered_info, None);

        // 5. Concept A titlebar Overlays ▾ popover toggle & active overlay count
        assert!(!s.overlays_popover_open);
        s.set_hovered_info(tip_a);
        s.toggle_overlays_popover();
        assert!(s.overlays_popover_open);
        assert_eq!(s.hovered_info, None);
        s.toggle_overlays_popover();
        assert!(!s.overlays_popover_open);

        assert_eq!(s.active_overlay_count(), 0);
        s.camera.exposure_warning = true;
        s.camera.focus_mask = true;
        s.camera.loupe_enabled = true;
        assert_eq!(s.active_overlay_count(), 3);
    }

    #[test]
    fn phase3_camera_profile_selection_strength_undo_mcp_and_raw_mode() {
        let mut s = AppState::default();

        // 1. Registry bounds for `profile_strength` match SCALAR_PARAMS
        let spec = find_scalar_param("profile_strength")
            .expect("profile_strength must be registered in SCALAR_PARAMS");
        assert_eq!((spec.min, spec.max, spec.default), (0.0, 1.0, 1.0));
        assert_eq!(scalar_bounds("profile_strength"), (0.0, 1.0));
        assert_eq!(scalar_range("profile_strength"), 0.0..=1.0);
        assert!((scalar_default("profile_strength") - 1.0).abs() < 1e-6);

        // 2. RAW decode mode mapping (`apple_raw` -> AppleNativeBoost, default -> Linear)
        assert_eq!(
            AppState::raw_decode_mode_for_recipe(&s.recipe),
            RawDecodeMode::Linear
        );
        let apple_recipe = DevelopRecipe {
            camera_profile: "apple_raw".to_owned(),
            ..DevelopRecipe::default()
        };
        assert_eq!(
            AppState::raw_decode_mode_for_recipe(&apple_recipe),
            RawDecodeMode::AppleNativeBoost
        );

        // 3. Selecting camera profiles records undo snapshots & propagates to MCP snapshot
        s.recipe.preset_name = Some("Warm Film".to_owned());
        s.set_camera_profile("classic_chrome");
        assert_eq!(s.recipe.camera_profile, "classic_chrome");
        assert!(s.recipe.preset_name.is_none());
        assert!(s.compiled_recipe.profile_active);
        assert!(s.history.can_undo());
        assert_eq!(
            s.mcp_shared_canvas.lock().unwrap().recipe.camera_profile,
            "classic_chrome"
        );
        assert_eq!(
            s.active_camera_profile_label(),
            ("Classic Chrome".to_owned(), false)
        );

        // Switching again + scrubbing `profile_strength` pushes clean undo snapshots
        s.set_camera_profile("pro_standard");
        assert_eq!(s.recipe.camera_profile, "pro_standard");
        assert_eq!(
            s.mcp_shared_canvas.lock().unwrap().recipe.camera_profile,
            "pro_standard"
        );

        s.update_slider_inner(false, true, |r| r.profile_strength = 0.72);
        s.update_slider_inner(true, true, |r| r.profile_strength = 0.65);
        assert!((s.recipe.profile_strength - 0.65).abs() < 1e-5);
        assert!((s.mcp_shared_canvas.lock().unwrap().recipe.profile_strength - 0.65).abs() < 1e-5);

        // Undo unwinds profile_strength -> pro_standard -> classic_chrome -> auto cleanly
        s.undo();
        assert!((s.recipe.profile_strength - 1.0).abs() < 1e-5);
        assert_eq!(s.recipe.camera_profile, "pro_standard");
        assert!((s.mcp_shared_canvas.lock().unwrap().recipe.profile_strength - 1.0).abs() < 1e-5);

        s.undo();
        assert_eq!(s.recipe.camera_profile, "classic_chrome");
        assert_eq!(
            s.mcp_shared_canvas.lock().unwrap().recipe.camera_profile,
            "classic_chrome"
        );

        s.undo();
        assert_eq!(s.recipe.camera_profile, "auto");
        assert_eq!(
            s.mcp_shared_canvas.lock().unwrap().recipe.camera_profile,
            "auto"
        );

        // 4. Hermetic external `.cube` profile import into temp dir activates user:<slug>
        let tmp_dir =
            std::env::temp_dir().join(format!("rune_darkroom_p3_profiles_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp_dir);
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let cube_src = tmp_dir.join("Studio_Warm.cube");
        let cube_contents = "\
TITLE \"Studio Warm\"
LUT_3D_SIZE 2
0.0 0.0 0.0
1.0 0.0 0.0
0.0 1.0 0.0
1.0 1.0 0.0
0.0 0.0 1.0
1.0 0.0 1.0
0.0 1.0 1.0
1.0 1.0 1.0
";
        std::fs::write(&cube_src, cube_contents).unwrap();
        let store_dir = tmp_dir.join("store");
        let desc = s
            .import_camera_profile_from_dir(&store_dir, &cube_src)
            .expect("hermetic .cube import must succeed");
        assert_eq!(desc.id, "user:studio_warm");
        assert_eq!(s.recipe.camera_profile, "user:studio_warm");
        assert_eq!(
            s.mcp_shared_canvas.lock().unwrap().recipe.camera_profile,
            "user:studio_warm"
        );
        assert!(
            s.user_profiles
                .iter()
                .any(|(d, _)| d.id == "user:studio_warm")
        );
        assert_eq!(s.active_camera_profile_label().0, "Studio Warm");
        let _ = std::fs::remove_dir_all(&tmp_dir);

        // 5. `active_raw_decode_mode` stays synced across reset_recipe, MCP edits, and variants
        s.set_camera_profile("apple_raw");
        assert_eq!(s.active_raw_decode_mode, RawDecodeMode::AppleNativeBoost);

        s.reset_recipe();
        assert_eq!(s.recipe.camera_profile, "auto");
        assert_eq!(s.active_raw_decode_mode, RawDecodeMode::Linear);

        let mcp_apple = DevelopRecipe {
            camera_profile: "apple_raw".to_owned(),
            ..DevelopRecipe::default()
        };
        let (changed, _) = s.apply_external_mcp_recipe(mcp_apple);
        assert!(changed);
        assert_eq!(s.recipe.camera_profile, "apple_raw");
        assert_eq!(s.active_raw_decode_mode, RawDecodeMode::AppleNativeBoost);

        // Variant creation/selection/deletion keeps `active_raw_decode_mode` synced
        let v2 = s.new_variant(Some("Neutral Linear".to_owned()));
        assert_eq!(v2, 1);
        assert_eq!(s.recipe.camera_profile, "auto");
        assert_eq!(s.active_raw_decode_mode, RawDecodeMode::Linear);

        assert!(s.select_variant(0));
        assert_eq!(s.recipe.camera_profile, "apple_raw");
        assert_eq!(s.active_raw_decode_mode, RawDecodeMode::AppleNativeBoost);

        assert!(s.delete_active_variant());
        assert_eq!(s.recipe.camera_profile, "auto");
        assert_eq!(s.active_raw_decode_mode, RawDecodeMode::Linear);
    }

    #[test]
    fn ai_select_tool_points_bbox_invert_and_undo() {
        const MASK_DIM: u32 = rune_editor_core::LAYER_MASK_TILE_SIZE as u32;
        let mut s = AppState {
            viewport_rect_pts: [800.0, 600.0],
            viewport_scale: 1.0,
            ..AppState::default()
        };

        // 1. Activating AiSelect switches to Layers tab and advertises shortcut W
        assert_eq!(ShortcutAction::AiSelect.default_key(), "w");
        s.set_canvas_tool(CanvasTool::AiSelect);
        assert_eq!(s.active_tool, CanvasTool::AiSelect);
        assert!(s.active_tool.is_retouch_or_mask());
        assert_eq!(s.active_tool.hud_badge(), "AI SELECT (W)");
        assert_eq!(s.active_tab, InspectorTab::Layers);

        // 2. Positive prompt point creates a 512×512 RasterMask layer, enables mask overlay, pushes undo & syncs MCP
        assert!(s.ai_select_add_point(0.50, 0.45, true));
        assert_eq!(s.ai_select_points.len(), 1);
        assert!(s.ai_select_points[0].positive);
        assert_eq!(s.recipe.layers.len(), 1);
        assert_eq!(s.active_layer_index, Some(0));
        assert_eq!(s.camera.mask_overlay_mode, 1);
        assert!(s.last_ai_mask_status.is_some());
        assert!(s.history.can_undo());
        assert_eq!(s.mcp_shared_canvas.lock().unwrap().recipe.layers.len(), 1);
        assert!(matches!(
            s.recipe.layers[0].masks[0].shape,
            MaskShape::RasterMask {
                width: MASK_DIM,
                height: MASK_DIM,
                ..
            }
        ));

        // 3. Negative prompt point refines the SAME layer rather than spawning a second layer
        assert!(s.ai_select_add_point(0.12, 0.18, false));
        assert_eq!(s.ai_select_points.len(), 2);
        assert!(!s.ai_select_points[1].positive);
        assert_eq!(s.recipe.layers.len(), 1);

        // 4. Shift+Drag produces a live bbox preview before release
        s.current_mods.shift = true;
        s.begin_canvas_tool_drag(260.0, 140.0);
        s.update_canvas_tool_drag(540.0, 460.0);
        assert!(
            s.ai_select_bbox_preview.is_some(),
            "Shift+Drag must populate live bounding-box preview"
        );
        let preview_box = s.ai_select_bbox_preview.unwrap();
        assert!(preview_box.is_valid());
        s.current_mods.shift = false;
        s.layer_drag = None;

        // 5. Committing bbox + toggling Invert updates the 512×512 RasterMask and pushes undo
        assert!(s.ai_select_set_bbox(preview_box));
        assert_eq!(s.ai_select_bbox, Some(preview_box.sanitized()));
        assert!(s.ai_select_bbox_preview.is_none());

        let mask_before_invert = s.recipe.layers[0].masks[0].shape.clone();
        assert!(s.ai_select_toggle_invert());
        assert!(s.ai_select_invert);
        let mask_after_invert = s.recipe.layers[0].masks[0].shape.clone();
        assert_ne!(
            mask_before_invert, mask_after_invert,
            "inverting interactive AI selection must invert the generated RasterMask"
        );

        // 6. Undo restores pre-invert mask cleanly
        s.undo();
        assert_eq!(s.recipe.layers[0].masks[0].shape, mask_before_invert);

        // 7. Clear Points & Done preserve the layer mask while clearing transient prompts and returning to Hand
        s.ai_select_clear_points();
        assert!(s.ai_select_points.is_empty());
        assert!(s.ai_select_bbox.is_none());
        assert!(!s.ai_select_invert);
        assert_eq!(s.recipe.layers.len(), 1);

        s.ai_select_done();
        assert_eq!(s.active_tool, CanvasTool::Hand);
        assert!(s.ai_select_target_layer.is_none());
        assert_eq!(s.recipe.layers.len(), 1);
    }

    #[test]
    fn settings_ai_models_config_license_gate_and_download_job_state() {
        let mut s = AppState::default();

        // 1. All 3 downloadable models are cataloged and effective engine readout is populated
        assert_eq!(s.ai_model_statuses.len(), ModelId::ALL.len());
        let readout = s.ai_effective_engine_readout();
        assert!(
            readout.contains("Subject:") && readout.contains("Interactive:"),
            "unexpected effective engine readout: {readout}"
        );

        // 2. SAM 3 license gate blocks download when unchecked and toggles cleanly across SAM 3 variants
        s.ai_model_config.accepted_licenses.clear();
        s.ai_license_toggles
            .insert(ModelId::Sam3TrackerQ4f16, false);
        s.ai_license_toggles.insert(ModelId::Sam3TrackerFp16, false);
        assert!(!s.is_ai_license_checked(ModelId::Sam3TrackerQ4f16));
        assert!(s.is_ai_license_checked(ModelId::Sam21HieraTinyFp16));
        let err = s
            .start_model_download(ModelId::Sam3TrackerQ4f16)
            .expect_err("SAM 3 download must be blocked until license checkbox is accepted");
        assert!(err.contains("Meta SAM License"));
        let job = s
            .model_download_job(ModelId::Sam3TrackerQ4f16)
            .expect("blocked download records error job state");
        assert!(matches!(job.phase, ModelDownloadPhase::Error(_)));

        s.toggle_ai_license_acceptance(ModelId::Sam3TrackerQ4f16);
        assert!(s.is_ai_license_checked(ModelId::Sam3TrackerQ4f16));
        assert!(s.is_ai_license_checked(ModelId::Sam3TrackerFp16));

        // 3. ModelDownloadJob progress & AtomicBool cancellation lifecycle
        let mut dl_job = ModelDownloadJob::with_phase(ModelDownloadPhase::Completed);
        assert!(!dl_job.is_active());
        assert!(!dl_job.is_cancel_requested());
        dl_job.apply_progress(ModelDownloadProgress {
            model_id: ModelId::Sam21HieraTinyFp16,
            file_index: 1,
            total_files: 4,
            file_name: "vision_encoder_fp16.onnx_data".to_owned(),
            bytes_downloaded: 33_500_000,
            total_bytes: 67_005_504,
            overall_fraction: 0.43,
            status: "Downloading vision_encoder_fp16.onnx_data".to_owned(),
        });
        assert!(dl_job.is_active());
        let shared_job = Arc::new(Mutex::new(dl_job));
        s.ai_download_jobs
            .jobs
            .lock()
            .unwrap()
            .insert(ModelId::Sam21HieraTinyFp16, shared_job);
        assert!(s.cancel_model_download(ModelId::Sam21HieraTinyFp16));
        assert!(
            s.model_download_job(ModelId::Sam21HieraTinyFp16)
                .unwrap()
                .is_cancel_requested()
        );

        // 4. Hermetic AiModelConfig save/load roundtrip into temp file
        let tmp_cfg = std::env::temp_dir().join(format!(
            "rune_darkroom_ai_cfg_test_{}.json",
            std::process::id()
        ));
        let mut cfg = AiModelConfig {
            active_engine: SegmentationEngine::Sam21Tiny,
            ..AiModelConfig::default()
        };
        cfg.accept_license(ModelId::Sam3TrackerQ4f16);
        rune_editor_core::save_ai_model_config_to(&tmp_cfg, &cfg).unwrap();
        let reloaded = rune_editor_core::load_ai_model_config_from(&tmp_cfg).unwrap();
        assert_eq!(reloaded.active_engine, SegmentationEngine::Sam21Tiny);
        assert!(reloaded.is_license_accepted(ModelId::Sam3TrackerQ4f16));

        // 5. P1 #1 regression: persisting preferred SAM 3 variant reloads disk `accepted_licenses`
        //    so a stale in-memory `ai_model_config` never wipes license acceptance written on disk.
        s.ai_model_config = AiModelConfig::default();
        assert!(
            !s.ai_model_config
                .is_license_accepted(ModelId::Sam3TrackerQ4f16)
        );
        s.persist_preferred_sam3_variant_at(&tmp_cfg, ModelId::Sam3TrackerFp16);
        let after_variant = rune_editor_core::load_ai_model_config_from(&tmp_cfg).unwrap();
        assert_eq!(
            after_variant.preferred_sam3_variant,
            ModelId::Sam3TrackerFp16
        );
        assert!(
            after_variant.is_license_accepted(ModelId::Sam3TrackerQ4f16),
            "persisting preferred SAM 3 variant must preserve accepted_licenses from disk"
        );
        let _ = std::fs::remove_file(&tmp_cfg);

        // 6. P1 #4 regression: dropping AppState cancels all active background download jobs.
        let drop_cancel_flag = {
            let doomed = AppState::default();
            let job = ModelDownloadJob::with_phase(ModelDownloadPhase::Downloading {
                fraction: 0.25,
                file_index: 0,
                total_files: 4,
                file_name: "vision_encoder_fp16.onnx_data".to_owned(),
                bytes_downloaded: 10_000_000,
                total_bytes: 67_005_504,
                status: "Downloading".to_owned(),
            });
            let flag = job.cancel.clone();
            doomed
                .ai_download_jobs
                .jobs
                .lock()
                .unwrap()
                .insert(ModelId::Sam21HieraTinyFp16, Arc::new(Mutex::new(job)));
            assert!(!flag.load(Ordering::Relaxed));
            drop(doomed);
            flag
        };
        assert!(
            drop_cancel_flag.load(Ordering::Relaxed),
            "dropping AppState must signal cancel on active download jobs"
        );
    }

    #[test]
    fn ai_select_reversed_bbox_photo_switch_and_layer_stack_invariants() {
        let mut s = AppState {
            viewport_rect_pts: [800.0, 600.0],
            viewport_scale: 1.0,
            ..AppState::default()
        };
        s.set_canvas_tool(CanvasTool::AiSelect);

        // 1. P1 #2 regression: reversed Shift+Drag directions (BR->TL, R->L, B->T) commit a valid
        //    sanitized PromptBox and never fall back to dropping a click point.
        for (start_xy, end_xy) in [
            ((540.0, 460.0), (260.0, 140.0)), // bottom-right -> top-left
            ((540.0, 140.0), (260.0, 460.0)), // top-right -> bottom-left
            ((260.0, 460.0), (540.0, 140.0)), // bottom-left -> top-right
        ] {
            s.ai_select_clear_points();
            s.current_mods.shift = true;
            s.begin_canvas_tool_drag(start_xy.0, start_xy.1);
            s.update_canvas_tool_drag(end_xy.0, end_xy.1);
            let preview = s
                .ai_select_bbox_preview
                .expect("reversed Shift+Drag must produce sanitized preview box");
            assert!(preview.is_valid(), "preview must be valid: {preview:?}");
            s.finish_canvas_tool_drag();
            s.current_mods.shift = false;
            assert!(
                s.ai_select_bbox.is_some_and(|b| b.is_valid()),
                "reversed Shift+Drag must commit a valid PromptBox"
            );
            assert!(
                s.ai_select_points.is_empty(),
                "reversed Shift+Drag must not fall back to adding a click point"
            );
        }

        // 2. P1 #3 regression: layer stack edits (select_layer, move_layer, remove_layer) keep
        //    ai_select_target_layer synchronized and never overwrite the wrong layer.
        assert_eq!(s.recipe.layers.len(), 1);
        assert_eq!(s.ai_select_target_layer, Some(0));
        let l1 = s
            .add_layer_with_shape("Second Layer".to_owned(), Some(MaskShape::Filled))
            .expect("second layer added");
        assert_eq!(l1, 1);
        // Selecting layer 1 clears old prompts and rebinds target to layer 1
        s.ai_select_add_point(0.5, 0.5, true);
        assert_eq!(s.ai_select_target_layer, Some(0));
        s.select_layer(Some(1));
        assert_eq!(s.ai_select_target_layer, Some(1));
        assert!(s.ai_select_points.is_empty());
        assert!(s.ai_select_bbox.is_none());

        // Moving layer 1 down to index 0 updates ai_select_target_layer 1 -> 0
        assert!(s.move_layer(1, -1));
        assert_eq!(s.ai_select_target_layer, Some(0));

        // Removing layer 0 clears ai_select_target_layer & prompts
        s.ai_select_points.push(PromptPoint::new(0.4, 0.4, true));
        assert!(s.remove_layer(0));
        assert_eq!(s.ai_select_target_layer, None);
        assert!(s.ai_select_points.is_empty());

        // Removing a lower-index layer decrements ai_select_target_layer
        let l_top = s
            .add_layer_with_shape("Top Layer".to_owned(), Some(MaskShape::Filled))
            .unwrap();
        assert_eq!(l_top, 1);
        s.select_layer(Some(1));
        assert_eq!(s.ai_select_target_layer, Some(1));
        assert!(s.remove_layer(0));
        assert_eq!(s.ai_select_target_layer, Some(0));

        // 3. P1 #3 regression: photo switch clears points, bbox, invert, target layer, and telemetry
        s.ai_select_add_point(0.5, 0.45, true);
        s.ai_select_invert = true;
        assert!(!s.ai_select_points.is_empty());
        assert!(s.last_ai_mask_status.is_some());
        s.reset_ai_select_for_photo_switch();
        assert!(s.ai_select_points.is_empty());
        assert!(s.ai_select_bbox.is_none());
        assert!(s.ai_select_bbox_preview.is_none());
        assert!(!s.ai_select_invert);
        assert!(s.ai_select_target_layer.is_none());
        assert!(s.last_ai_mask_status.is_none());
    }
}
