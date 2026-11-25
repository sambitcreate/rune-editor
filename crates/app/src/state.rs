//! Application state for the Rune Darkroom photo editor.

use std::path::PathBuf;
use std::sync::Arc;

use rune_editor_core::{
    DevelopRecipe, HistogramData, HistoryStack, StylePreset, ViewportCamera, builtin_presets,
    compute_histogram,
};
use rune_editor_decode::{LinearImage, generate_demo_hdr_scene, load_file};

/// Effect slot mark ID for the central GPU photo viewport.
pub const PHOTO_VIEWPORT_MARK: u32 = 1;

pub struct AppState {
    pub image: Arc<LinearImage>,
    /// Incremented whenever `image` changes so the GPU compositor re-uploads once.
    pub image_generation: u64,
    pub recipe: DevelopRecipe,
    pub history: HistoryStack,
    pub camera: ViewportCamera,
    pub histogram: HistogramData,
    pub presets: Vec<StylePreset>,
    /// Snapshot captured when a continuous slider drag starts (for clean single-step undo).
    pub drag_undo_snapshot: Option<DevelopRecipe>,
    /// Last pointer position for viewport drag panning.
    pub last_drag_pos: Option<(f32, f32)>,
    /// Whether the user is dragging a file over the viewport.
    pub drop_hover: bool,
    /// Brief status message shown in the bottom bar.
    pub status_message: Option<String>,
}

impl Default for AppState {
    fn default() -> Self {
        let image = Arc::new(generate_demo_hdr_scene(1600, 1066));
        let recipe = DevelopRecipe::default();
        let histogram = compute_histogram(&image.histogram_proxy, &recipe);
        Self {
            image,
            image_generation: 1,
            recipe,
            history: HistoryStack::default(),
            camera: ViewportCamera::default(),
            histogram,
            presets: builtin_presets(),
            drag_undo_snapshot: None,
            last_drag_pos: None,
            drop_hover: false,
            status_message: Some("Ready — drag & drop a photo or press Open to load".to_owned()),
        }
    }
}

impl AppState {
    pub fn recompute_histogram(&mut self) {
        self.histogram = compute_histogram(&self.image.histogram_proxy, &self.recipe);
    }

    pub fn update_slider(&mut self, done: bool, mutate: impl FnOnce(&mut DevelopRecipe)) {
        if self.drag_undo_snapshot.is_none() {
            self.drag_undo_snapshot = Some(self.recipe.clone());
        }
        mutate(&mut self.recipe);
        self.recipe.preset_name = None;
        self.recompute_histogram();
        if done
            && let Some(prev) = self.drag_undo_snapshot.take()
            && prev != self.recipe
        {
            self.history.push(prev);
        }
    }

    pub fn apply_preset(&mut self, preset_id: &str) {
        let Some(preset) = self.presets.iter().find(|p| p.id == preset_id).cloned() else {
            return;
        };
        if self.recipe != preset.recipe {
            self.history.push(self.recipe.clone());
            self.recipe = preset.recipe;
            self.recompute_histogram();
            self.status_message = Some(format!("Applied style: {}", preset.name));
        }
    }

    pub fn reset_recipe(&mut self) {
        let neutral = DevelopRecipe::default();
        if self.recipe != neutral {
            self.history.push(self.recipe.clone());
            self.recipe = neutral;
            self.recompute_histogram();
            self.status_message = Some("Reset all adjustments to Neutral".to_owned());
        }
    }

    pub fn undo(&mut self) {
        if self.history.undo(&mut self.recipe) {
            self.recompute_histogram();
            self.status_message = Some("Undid last adjustment".to_owned());
        }
    }

    pub fn redo(&mut self) {
        if self.history.redo(&mut self.recipe) {
            self.recompute_histogram();
            self.status_message = Some("Redid adjustment".to_owned());
        }
    }

    pub fn save_sidecar(&mut self) {
        let Some(path) = self.image.source_path.as_deref() else {
            self.status_message = Some("Open a photo from disk to save a .rune-edit.json sidecar".to_owned());
            return;
        };
        match self.recipe.save_sidecar(path) {
            Ok(sidecar) => {
                let name = sidecar
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "sidecar".to_owned());
                self.status_message = Some(format!("Saved sidecar {name}"));
            }
            Err(err) => {
                self.status_message = Some(format!("Sidecar save failed: {err}"));
            }
        }
    }

    pub fn open_image_path(&mut self, path: PathBuf) {
        match load_file(&path) {
            Ok(decoded) => {
                let restored_recipe = DevelopRecipe::load_sidecar(&path)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let name = decoded.display_name.clone();
                let dims = format!("{}×{}", decoded.width, decoded.height);
                self.image = Arc::new(decoded);
                self.image_generation = self.image_generation.wrapping_add(1);
                self.recipe = restored_recipe;
                self.history.clear();
                self.camera.reset();
                self.recompute_histogram();
                self.status_message = Some(format!("Loaded {name} ({dims})"));
            }
            Err(err) => {
                self.status_message = Some(format!("Could not open image: {err}"));
            }
        }
    }
}
