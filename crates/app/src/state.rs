//! Application state, session catalog orchestration, undo/redo, demo filmstrip,
//! background neighbor prefetching, and macOS document-state synchronization.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rune::{AppHandle, LiveRegion, Pixels, Tx};
use rune_editor_core::{
    CompiledRecipe, CullFilter, CullFlag, DevelopRecipe, HistogramData, HistoryStack, PhotoMeta,
    SessionCatalog, SessionEntry, StylePreset, ToneCurves, ViewportCamera, builtin_presets,
    compute_histogram, default_curve_points, linear_to_srgb,
};
use rune_editor_decode::{
    ExportFormat, LinearImage, ThumbnailRgba8, export_image, generate_demo_hdr_scene,
    generate_demo_thumbnail, load_file, load_from_memory, load_thumbnail, scan_photo_folder,
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
    Presets,
}

impl InspectorTab {
    pub const ALL: [Self; 4] = [Self::Light, Self::Color, Self::Detail, Self::Presets];
    pub const LABELS: [&'static str; 4] = ["Light", "Color", "Detail", "Presets"];

    pub fn index(self) -> usize {
        match self {
            Self::Light => 0,
            Self::Color => 1,
            Self::Detail => 2,
            Self::Presets => 3,
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

pub struct AppState {
    pub image: Arc<LinearImage>,
    /// Incremented whenever `image` changes so the GPU compositor re-uploads once.
    pub image_generation: u64,
    /// Incremented whenever `recipe.curves` changes so the GPU uploads the 256x1 LUT once.
    pub curve_generation: u64,
    pub recipe: DevelopRecipe,
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
    pub curve_open: bool,

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
        let histogram = compute_histogram(&image.histogram_proxy, &recipe);

        let (catalog, thumbnails) = build_demo_session(&image, &presets);

        Self {
            image,
            image_generation: 1,
            curve_generation: 1,
            recipe: recipe.clone(),
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
            curve_open: false,
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
    pub fn with_handle(handle: AppHandle<AppState>) -> Self {
        Self {
            app_handle: Some(handle),
            ..Self::default()
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.recipe != self.saved_recipe
    }

    pub fn bump_curve_generation(&mut self) {
        self.curve_generation = self.curve_generation.wrapping_add(1);
    }

    pub fn recompute_histogram(&mut self) {
        self.histogram = compute_histogram(&self.image.histogram_proxy, &self.recipe);
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

    pub fn update_slider(&mut self, done: bool, mutate: impl FnOnce(&mut DevelopRecipe)) {
        if self.drag_undo_snapshot.is_none() {
            self.drag_undo_snapshot = Some(self.recipe.clone());
        }
        let curves_before = self.recipe.curves.clone();
        mutate(&mut self.recipe);
        self.recipe.preset_name = None;
        if self.recipe.curves != curves_before {
            self.bump_curve_generation();
        }
        self.catalog.update_active_recipe(self.recipe.clone());
        self.recompute_histogram();
        if done
            && let Some(prev) = self.drag_undo_snapshot.take()
            && prev != self.recipe
        {
            self.history.push(prev);
        }
    }

    pub fn update_slider_tx(
        &mut self,
        done: bool,
        tx: &mut Tx,
        mutate: impl FnOnce(&mut DevelopRecipe),
    ) {
        self.update_slider(done, mutate);
        if done {
            self.sync_window_state(tx);
        }
    }

    pub fn commit_recipe_change(&mut self, next: DevelopRecipe, status: impl Into<String>) {
        if self.recipe != next {
            let curves_changed = self.recipe.curves != next.curves;
            self.history.push(self.recipe.clone());
            self.recipe = next;
            if curves_changed {
                self.bump_curve_generation();
            }
            self.catalog.update_active_recipe(self.recipe.clone());
            self.recompute_histogram();
            self.status_message = Some(status.into());
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
        let curves_before = self.recipe.curves.clone();
        if self.history.undo(&mut self.recipe) {
            if self.recipe.curves != curves_before {
                self.bump_curve_generation();
            }
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
        let curves_before = self.recipe.curves.clone();
        if self.history.redo(&mut self.recipe) {
            if self.recipe.curves != curves_before {
                self.bump_curve_generation();
            }
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
        // Check if this photo is inside a folder we should populate into the filmstrip
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

                // If opening a standalone file while in demo mode, switch catalog to this file's parent folder if valid.
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
                self.bump_curve_generation();
                self.history.clear();
                self.camera.reset();
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
            self.bump_curve_generation();
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
                self.bump_curve_generation();
                self.history.clear();
                self.camera.reset();
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
            _ => (ExportFormat::Jpeg { quality: 92 }, "JPEG (92%)"),
        };
        match export_image(&self.image, &self.recipe, &dest, fmt) {
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
