//! Application state, session catalog orchestration, undo/redo, Speed Edit,
//! live cursor color readout, interactive crop, and macOS document-state sync.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rune::{AppHandle, LiveRegion, Pixels, Tx};
use rune_editor_core::{
    CompiledRecipe, CropAspect, CullFilter, CullFlag, DevelopRecipe, HistogramData, HistoryStack,
    PhotoMeta, SessionCatalog, SessionEntry, StylePreset, ToneCurves, ViewportCamera,
    auto_exposure_and_tone, auto_levels, auto_white_balance, builtin_presets,
    compute_histogram_compiled, default_curve_points, filmic_tonemap, linear_to_srgb,
    pick_white_balance, rgb_to_hsl_and_lab,
};
use rune_editor_decode::{
    ExportFormat, ExportOptions, LinearImage, ThumbnailRgba8, export_image_with_options,
    generate_demo_hdr_scene, generate_demo_thumbnail, load_file, load_from_memory, load_thumbnail,
    scan_photo_folder,
};

const SAMPLE_PORTRAIT_BYTES: &[u8] =
    include_bytes!("../../../assets/samples/editorial_portrait.jpg");

use crate::menu::build_menu_items;

/// Effect slot mark ID for the central GPU photo viewport.
pub const PHOTO_VIEWPORT_MARK: u32 = 1;

/// Right inspector tab selection (Capture One / Lightroom darkroom layout).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InspectorTab {
    #[default]
    Light,
    Color,
    Detail,
    Crop,
    Presets,
}

impl InspectorTab {
    pub const ALL: [Self; 5] = [
        Self::Light,
        Self::Color,
        Self::Detail,
        Self::Crop,
        Self::Presets,
    ];
    pub const LABELS: [&'static str; 5] = ["Light", "Color", "Detail", "Crop", "Style"];

    pub fn index(self) -> usize {
        match self {
            Self::Light => 0,
            Self::Color => 1,
            Self::Detail => 2,
            Self::Crop => 3,
            Self::Presets => 4,
        }
    }

    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or(Self::Light)
    }
}

/// Active channel inside the interactive Tone Curve Editor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CurveChannel {
    #[default]
    Luma,
    Red,
    Green,
    Blue,
}

impl CurveChannel {
    pub const ALL: [Self; 4] = [Self::Luma, Self::Red, Self::Green, Self::Blue];

    pub fn label(self) -> &'static str {
        match self {
            Self::Luma => "Luma",
            Self::Red => "Red",
            Self::Green => "Green",
            Self::Blue => "Blue",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Luma => "Luma",
            Self::Red => "R",
            Self::Green => "G",
            Self::Blue => "B",
        }
    }

    pub fn points(self, curves: &ToneCurves) -> &[[f32; 2]] {
        match self {
            Self::Luma => &curves.luma,
            Self::Red => &curves.red,
            Self::Green => &curves.green,
            Self::Blue => &curves.blue,
        }
    }

    pub fn points_mut(self, curves: &mut ToneCurves) -> &mut Vec<[f32; 2]> {
        match self {
            Self::Luma => &mut curves.luma,
            Self::Red => &mut curves.red,
            Self::Green => &mut curves.green,
            Self::Blue => &mut curves.blue,
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
            Self::Temperature => format!("{:+.0}", r.temperature * 100.0),
            Self::Tint => format!("{:+.0}", r.tint * 100.0),
        }
    }

    /// Applies a normalized scrub delta (`~1.0` ≈ full comfortable sweep) and returns
    /// `true` if the recipe parameter changed.
    pub fn apply_delta(self, r: &mut DevelopRecipe, delta: f32) -> bool {
        if !delta.is_finite() || delta.abs() < 1e-6 {
            return false;
        }
        let (slot, min, max, scale) = match self {
            Self::Exposure => (&mut r.exposure, -4.0, 4.0, 3.0),
            Self::Contrast => (&mut r.contrast, -1.0, 1.0, 1.0),
            Self::Brightness => (&mut r.brightness, -1.0, 1.0, 1.0),
            Self::Saturation => (&mut r.saturation, -1.0, 1.0, 1.0),
            Self::Highlights => (&mut r.highlights, -1.0, 1.0, 1.0),
            Self::Shadows => (&mut r.shadows, -1.0, 1.0, 1.0),
            Self::Whites => (&mut r.whites, -1.0, 1.0, 1.0),
            Self::Clarity => (&mut r.clarity, -1.0, 1.0, 1.0),
            Self::Temperature => (&mut r.temperature, -1.0, 1.0, 1.0),
            Self::Tint => (&mut r.tint, -1.0, 1.0, 1.0),
        };
        let prev = *slot;
        *slot = (prev + delta * scale).clamp(min, max);
        (*slot - prev).abs() > 1e-6
    }
}

/// Live Capture One pixel probe values under the cursor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorColorReadout {
    pub rgb8: [u8; 3],
    pub hsl: [f32; 3],
    pub lab: [f32; 3],
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
    pub dragging_curve_pt: Option<usize>,
    pub selected_hsl_band: usize,
    pub show_inspector: bool,
    pub show_filmstrip: bool,
    pub wb_open: bool,
    pub tone_open: bool,
    pub levels_open: bool,
    pub curve_open: bool,
    pub bw_open: bool,

    // Interactive Capture One tools & live probe state
    pub wb_eyedropper_active: bool,
    pub speed_edit: Option<SpeedEditParam>,
    pub speed_edit_snapshot: Option<DevelopRecipe>,
    pub viewport_rect_pts: [f32; 2],
    pub viewport_scale: f32,
    pub cursor_readout: Option<CursorColorReadout>,
    pub crop_drag: Option<(CropDragHandle, [f32; 2], [f32; 4])>,

    // Folder / session catalog & filmstrip culling state
    pub catalog: SessionCatalog,
    pub demo_mode: bool,
    pub thumbnails: HashMap<PathBuf, Pixels>,
    pub prefetch_cache: Arc<Mutex<HashMap<PathBuf, Arc<LinearImage>>>>,

    // Pointer / drag & drop interaction state
    pub drag_undo_snapshot: Option<DevelopRecipe>,
    pub last_drag_pos: Option<(f32, f32)>,
    pub drop_hover: bool,

    // Status, modal sheets & native menu handle
    pub status_message: Option<String>,
    pub show_about: bool,
    pub confirm_close: bool,
    pub app_handle: Option<AppHandle<AppState>>,
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
            dragging_curve_pt: None,
            selected_hsl_band: 0,
            show_inspector: true,
            show_filmstrip: false,
            wb_open: true,
            tone_open: true,
            levels_open: false,
            curve_open: false,
            bw_open: false,
            wb_eyedropper_active: false,
            speed_edit: None,
            speed_edit_snapshot: None,
            viewport_rect_pts: [0.0, 0.0],
            viewport_scale: 1.0,
            cursor_readout: None,
            crop_drag: None,
            catalog,
            demo_mode: true,
            thumbnails,
            prefetch_cache: Arc::new(Mutex::new(HashMap::new())),
            drag_undo_snapshot: None,
            last_drag_pos: None,
            drop_hover: false,
            status_message: Some("Ready — drop a RAW/photo or press ⌘O to open".to_owned()),
            show_about: false,
            confirm_close: false,
            app_handle: None,
        }
    }
}

impl AppState {
    pub fn is_dirty(&self) -> bool {
        self.recipe != self.saved_recipe
    }

    pub fn recompute_histogram(&mut self) {
        self.compiled_recipe = CompiledRecipe::from_recipe(&self.recipe);
        self.histogram =
            compute_histogram_compiled(&self.image.histogram_proxy, &self.compiled_recipe);
    }

    /// Clears in-flight pointer/keyboard interaction state so menu actions, undo/redo,
    /// or photo switches never leak stale undo snapshots or stuck drags.
    pub fn clear_transient_interactions(&mut self) {
        self.speed_edit = None;
        self.speed_edit_snapshot = None;
        self.crop_drag = None;
        self.drag_undo_snapshot = None;
        self.dragging_curve_pt = None;
        self.last_drag_pos = None;
    }

    /// Syncs macOS `NSWindow.documentEdited` and live native menu enabled/checked states.
    pub fn sync_window_state(&self, tx: &mut Tx) {
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

    pub fn auto_white_balance_tx(&mut self, tx: &mut Tx) {
        let (temp, tint) = auto_white_balance(&self.image.histogram_proxy);
        let mut next = self.recipe.clone();
        next.temperature = temp;
        next.tint = tint;
        next.preset_name = None;
        let msg = format!(
            "Auto WB (Temp {:+.0}, Tint {:+.0})",
            temp * 100.0,
            tint * 100.0
        );
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
            self.camera.split_compare = false;
            self.active_tab = InspectorTab::Crop;
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
        next.preset_name = None;
        self.wb_eyedropper_active = false;
        self.commit_recipe_change(
            next,
            format!(
                "Picked WB (Temp {:+.0}, Tint {:+.0})",
                temp * 100.0,
                tint * 100.0
            ),
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
        let Some([su, sv]) = self.viewport_local_to_source_uv(local_x, local_y) else {
            self.cursor_readout = None;
            return;
        };
        let src_lin = self.sample_raw_pixel_linear(su, sv);
        let graded_lin = if self.camera.show_original
            || (self.camera.split_compare && slot_u < self.camera.split_pos)
        {
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
        let w = self.viewport_rect_pts[0];
        let h = self.viewport_rect_pts[1];
        if w <= 1.0 || h <= 1.0 {
            return None;
        }
        let scale = self.viewport_scale.max(1.0);
        let slot_uv = [local_x / w, local_y / h];
        let slot_size = [w * scale, h * scale];
        self.recipe.geometry.viewport_to_source_uv(
            slot_uv,
            slot_size,
            [self.image.width, self.image.height],
            &self.camera,
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
        let label = format!("Applied style: {}", preset.name);
        self.commit_recipe_change(preset.recipe, label);
    }

    pub fn apply_preset_tx(&mut self, preset_id: &str, tx: &mut Tx) {
        self.apply_preset(preset_id);
        self.sync_window_state(tx);
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

    pub fn paste_adjustments_tx(&mut self, tx: &mut Tx) {
        let Some(copied) = self.catalog.paste_adjustments() else {
            self.status_message = Some("No adjustments copied yet".to_owned());
            return;
        };
        self.commit_recipe_change(copied, "Pasted develop adjustments");
        self.sync_window_state(tx);
        tx.announce("Pasted adjustments", LiveRegion::Polite);
    }

    pub fn save_sidecar_tx(&mut self, tx: &mut Tx) {
        let Some(path) = self.image.source_path.as_deref() else {
            self.status_message =
                Some("Open a photo from disk to save a .rune-edit.json sidecar".to_owned());
            return;
        };
        let meta = self
            .catalog
            .active_meta()
            .cloned()
            .map(|mut m| {
                m.recipe = self.recipe.clone();
                m
            })
            .unwrap_or_else(|| PhotoMeta {
                rating: 0,
                flag: CullFlag::None,
                recipe: self.recipe.clone(),
            });
        match meta.save_sidecar(path) {
            Ok(sidecar) => {
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
        let cached = self
            .prefetch_cache
            .lock()
            .ok()
            .and_then(|c| c.get(&path).cloned());

        let decoded_res = match cached {
            Some(img) => Ok(img),
            None => load_file(&path).map(Arc::new),
        };

        match decoded_res {
            Ok(decoded) => {
                let restored_meta = PhotoMeta::load_sidecar(&path)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
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
                self.recipe = restored_meta.recipe.clone();
                self.saved_recipe = restored_meta.recipe;
                self.clear_transient_interactions();
                self.history.clear();
                self.camera.reset();
                self.wb_eyedropper_active = false;
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

    pub fn switch_catalog_index_tx(&mut self, index: usize, tx: &mut Tx) {
        if self.catalog.active_index == Some(index) {
            return;
        }
        self.catalog.update_active_recipe(self.recipe.clone());
        if self.catalog.select_index(index).is_some() {
            self.load_active_catalog_photo_tx(tx);
        }
    }

    pub fn select_next_photo_tx(&mut self, tx: &mut Tx) {
        self.catalog.update_active_recipe(self.recipe.clone());
        if self.catalog.select_next().is_some() {
            self.load_active_catalog_photo_tx(tx);
        }
    }

    pub fn select_prev_photo_tx(&mut self, tx: &mut Tx) {
        self.catalog.update_active_recipe(self.recipe.clone());
        if self.catalog.select_prev().is_some() {
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
            self.wb_eyedropper_active = false;
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
        let cached = self
            .prefetch_cache
            .lock()
            .ok()
            .and_then(|c| c.get(&path).cloned());
        let decoded_res = match cached {
            Some(img) => Ok(img),
            None => load_file(&path).map(Arc::new),
        };
        match decoded_res {
            Ok(decoded) => {
                let name = decoded.display_name.clone();
                let dims = format!("{}×{}", decoded.width, decoded.height);
                self.image = decoded;
                self.image_generation = self.image_generation.wrapping_add(1);
                self.recipe = entry.meta.recipe.clone();
                self.saved_recipe = entry.meta.recipe;
                self.clear_transient_interactions();
                self.history.clear();
                self.camera.reset();
                self.wb_eyedropper_active = false;
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
            self.load_active_catalog_photo_tx(tx);
        }
    }

    pub fn set_entry_rating_tx(&mut self, index: usize, rating: u8, tx: &mut Tx) {
        if self.catalog.set_rating(index, rating) {
            if !self.demo_mode {
                let _ = self.catalog.save_entry_sidecar(index);
            }
            let stars = if rating == 0 {
                "Cleared rating".to_owned()
            } else {
                format!("Rated {rating}★")
            };
            self.status_message = Some(stars.clone());
            tx.announce(stars, LiveRegion::Polite);
        }
    }

    pub fn set_active_rating_tx(&mut self, rating: u8, tx: &mut Tx) {
        if let Some(idx) = self.catalog.active_index {
            self.set_entry_rating_tx(idx, rating, tx);
        }
    }

    pub fn toggle_active_flag_tx(&mut self, flag: CullFlag, tx: &mut Tx) {
        if let Some(next_flag) = self.catalog.toggle_active_flag(flag) {
            if !self.demo_mode {
                let _ = self.catalog.save_active_sidecar();
            }
            let msg = match next_flag {
                CullFlag::Pick => "Flagged as Pick",
                CullFlag::Reject => "Flagged as Reject",
                CullFlag::None => "Cleared culling flag",
            };
            self.status_message = Some(msg.to_owned());
            tx.announce(msg, LiveRegion::Polite);
        }
    }

    pub fn export_to_path_tx(&mut self, dest: PathBuf, tx: &mut Tx) {
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

    /// Prefetches adjacent catalog images on a background thread for instant `←`/`→` switching.
    fn spawn_prefetch_neighbors(&self) {
        if self.demo_mode {
            return;
        }
        let neighbors = self.catalog.neighbor_paths();
        if neighbors.is_empty() {
            return;
        }
        let cache = Arc::clone(&self.prefetch_cache);
        std::thread::spawn(move || {
            for path in neighbors {
                let already = cache.lock().map(|c| c.contains_key(&path)).unwrap_or(true);
                if already {
                    continue;
                }
                if let Ok(img) = load_file(&path)
                    && let Ok(mut guard) = cache.lock()
                {
                    if guard.len() > 8 {
                        guard.clear();
                    }
                    guard.insert(path, Arc::new(img));
                }
            }
        });
    }
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

    let frames: [(&str, &str, u8, CullFlag); 5] = [
        ("Editorial_Portrait.jpg", "neutral", 5, CullFlag::Pick),
        ("Morning_Coffee_Warm.jpg", "warm_film", 4, CullFlag::Pick),
        (
            "Classic_Chrome_Film.jpg",
            "classic_chrome",
            4,
            CullFlag::None,
        ),
        ("Editorial_Mono.jpg", "editorial_bw", 3, CullFlag::None),
        ("Soft_Dusk_Tone.jpg", "moody_dusk", 2, CullFlag::Reject),
    ];

    let mut entries = Vec::with_capacity(frames.len());
    let mut thumbs = HashMap::with_capacity(frames.len());
    for (name, preset_id, rating, flag) in frames {
        let path = PathBuf::from(format!("demo/{name}"));
        let recipe = find_recipe(preset_id);
        let px = render_graded_demo_pixels(image, &recipe, 180);
        thumbs.insert(path.clone(), px);
        entries.push(SessionEntry {
            path,
            meta: PhotoMeta {
                rating,
                flag,
                recipe,
            },
        });
    }

    let catalog = SessionCatalog {
        folder: None,
        entries,
        active_index: Some(0),
        filter: CullFilter::All,
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

        // Extreme scrub clamps safely to +/-4.0 EV without NaN/Inf.
        s.scrub_speed_edit(100.0);
        assert_eq!(s.recipe.exposure, 4.0);
        assert!(!s.scrub_speed_edit(f32::NAN));
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
}
