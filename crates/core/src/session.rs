//! Folder / Session Catalog, Culling, Rating, Copy/Paste Adjustments, and Sidecar Persistence.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{CoreError, DevelopRecipe};

/// Culling pick/reject flag for a photograph in a session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CullFlag {
    #[default]
    None,
    Pick,
    Reject,
}

/// Per-photo catalog metadata persisted alongside the photo in `.rune-edit.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PhotoMeta {
    /// Star rating in `0..=5` (`0` = unrated).
    #[serde(default)]
    pub rating: u8,
    /// Culling pick/reject state.
    #[serde(default)]
    pub flag: CullFlag,
    /// Non-destructive develop recipe for this photograph.
    #[serde(default)]
    pub recipe: DevelopRecipe,
}

impl PhotoMeta {
    /// Serializes `PhotoMeta` to the `.rune-edit.json` sidecar next to `photo_path`.
    pub fn save_sidecar(&self, photo_path: &Path) -> Result<PathBuf, CoreError> {
        let sidecar = DevelopRecipe::sidecar_path_for(photo_path);
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&sidecar, json)?;
        Ok(sidecar)
    }

    /// Loads `PhotoMeta` from the `.rune-edit.json` sidecar next to `photo_path`.
    ///
    /// Backward-compatible with legacy sidecars containing a bare `DevelopRecipe` JSON object.
    pub fn load_sidecar(photo_path: &Path) -> Result<Option<Self>, CoreError> {
        let sidecar = DevelopRecipe::sidecar_path_for(photo_path);
        if !sidecar.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(sidecar)?;
        Self::from_sidecar_json(&text).map(Some)
    }

    /// Parses either a `PhotoMeta` JSON document or a legacy flat `DevelopRecipe` JSON document.
    pub fn from_sidecar_json(json: &str) -> Result<Self, CoreError> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        if value.get("recipe").is_some() {
            let mut meta: Self = serde_json::from_value(value)?;
            meta.rating = meta.rating.min(5);
            Ok(meta)
        } else {
            let recipe: DevelopRecipe = serde_json::from_value(value)?;
            Ok(Self {
                rating: 0,
                flag: CullFlag::None,
                recipe,
            })
        }
    }
}

/// Active filmstrip culling filter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CullFilter {
    #[default]
    All,
    PicksOnly,
    MinRating(u8),
}

impl CullFilter {
    /// Returns `true` when `meta` satisfies this filter.
    pub fn matches(&self, meta: &PhotoMeta) -> bool {
        match *self {
            Self::All => true,
            Self::PicksOnly => meta.flag == CullFlag::Pick,
            Self::MinRating(min) => meta.rating >= min,
        }
    }
}

/// A single photograph entry tracked in a `SessionCatalog`.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionEntry {
    pub path: PathBuf,
    pub meta: PhotoMeta,
}

/// Folder / session catalog managing photo paths, active index, culling filters,
/// adjustment clipboard (Copy/Paste Adjustments), and `.rune-edit.json` sidecars.
#[derive(Clone, Debug, Default)]
pub struct SessionCatalog {
    pub folder: Option<PathBuf>,
    pub entries: Vec<SessionEntry>,
    pub active_index: Option<usize>,
    pub filter: CullFilter,
    pub clipboard: Option<DevelopRecipe>,
}

impl SessionCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a session catalog from a list of photo paths, automatically loading
    /// any existing `.rune-edit.json` sidecars.
    pub fn from_paths(folder: Option<PathBuf>, paths: Vec<PathBuf>) -> Self {
        let mut catalog = Self::new();
        catalog.load_paths(folder, paths);
        catalog
    }

    /// Replaces the catalog's entries with `paths`, loading sidecars where available.
    pub fn load_paths(&mut self, folder: Option<PathBuf>, paths: Vec<PathBuf>) {
        self.folder = folder;
        self.entries = paths
            .into_iter()
            .map(|path| {
                let meta = PhotoMeta::load_sidecar(&path)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                SessionEntry { path, meta }
            })
            .collect();
        self.filter = CullFilter::All;
        self.active_index = if self.entries.is_empty() {
            None
        } else {
            Some(0)
        };
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns indices into `self.entries` that pass the active `CullFilter`.
    pub fn filtered_indices(&self) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| self.filter.matches(&entry.meta).then_some(idx))
            .collect()
    }

    pub fn active_entry(&self) -> Option<&SessionEntry> {
        self.active_index.and_then(|idx| self.entries.get(idx))
    }

    pub fn active_entry_mut(&mut self) -> Option<&mut SessionEntry> {
        self.active_index.and_then(|idx| self.entries.get_mut(idx))
    }

    pub fn active_path(&self) -> Option<&Path> {
        self.active_entry().map(|e| e.path.as_path())
    }

    pub fn active_meta(&self) -> Option<&PhotoMeta> {
        self.active_entry().map(|e| &e.meta)
    }

    pub fn active_meta_mut(&mut self) -> Option<&mut PhotoMeta> {
        self.active_entry_mut().map(|e| &mut e.meta)
    }

    /// Updates the active filter and snaps `active_index` into the filtered view if needed.
    pub fn set_filter(&mut self, filter: CullFilter) {
        self.filter = filter;
        let visible = self.filtered_indices();
        if visible.is_empty() {
            self.active_index = None;
            return;
        }
        if let Some(cur) = self.active_index
            && visible.contains(&cur)
        {
            return;
        }
        self.active_index = visible.first().copied();
    }

    /// Selects a specific entry by raw index in `self.entries`.
    pub fn select_index(&mut self, index: usize) -> Option<&SessionEntry> {
        if index < self.entries.len() {
            self.active_index = Some(index);
            self.entries.get(index)
        } else {
            None
        }
    }

    /// Advances to the next visible photo under the current `CullFilter`.
    pub fn select_next(&mut self) -> Option<usize> {
        let visible = self.filtered_indices();
        if visible.is_empty() {
            self.active_index = None;
            return None;
        }
        let next = match self.active_index {
            Some(cur) => {
                if let Some(pos) = visible.iter().position(|&i| i == cur) {
                    if pos + 1 < visible.len() {
                        visible[pos + 1]
                    } else {
                        return None;
                    }
                } else {
                    visible[0]
                }
            }
            None => visible[0],
        };
        self.active_index = Some(next);
        Some(next)
    }

    /// Moves to the previous visible photo under the current `CullFilter`.
    pub fn select_prev(&mut self) -> Option<usize> {
        let visible = self.filtered_indices();
        if visible.is_empty() {
            self.active_index = None;
            return None;
        }
        let prev = match self.active_index {
            Some(cur) => {
                if let Some(pos) = visible.iter().position(|&i| i == cur) {
                    if pos > 0 {
                        visible[pos - 1]
                    } else {
                        return None;
                    }
                } else {
                    visible[0]
                }
            }
            None => visible[0],
        };
        self.active_index = Some(prev);
        Some(prev)
    }

    /// Returns adjacent visible photo paths (previous and next) for background prefetching.
    pub fn neighbor_paths(&self) -> Vec<PathBuf> {
        let Some(cur) = self.active_index else {
            return Vec::new();
        };
        let visible = self.filtered_indices();
        let Some(pos) = visible.iter().position(|&i| i == cur) else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(2);
        if pos > 0 {
            out.push(self.entries[visible[pos - 1]].path.clone());
        }
        if pos + 1 < visible.len() {
            out.push(self.entries[visible[pos + 1]].path.clone());
        }
        out
    }

    pub fn set_rating(&mut self, index: usize, rating: u8) -> bool {
        let Some(entry) = self.entries.get_mut(index) else {
            return false;
        };
        entry.meta.rating = rating.min(5);
        true
    }

    pub fn set_active_rating(&mut self, rating: u8) -> bool {
        let Some(idx) = self.active_index else {
            return false;
        };
        self.set_rating(idx, rating)
    }

    pub fn set_flag(&mut self, index: usize, flag: CullFlag) -> bool {
        let Some(entry) = self.entries.get_mut(index) else {
            return false;
        };
        entry.meta.flag = flag;
        true
    }

    pub fn set_active_flag(&mut self, flag: CullFlag) -> bool {
        let Some(idx) = self.active_index else {
            return false;
        };
        self.set_flag(idx, flag)
    }

    /// Toggles `flag` on the active photo (if already set to `flag`, clears to `CullFlag::None`).
    pub fn toggle_active_flag(&mut self, flag: CullFlag) -> Option<CullFlag> {
        let entry = self.active_entry_mut()?;
        entry.meta.flag = if entry.meta.flag == flag {
            CullFlag::None
        } else {
            flag
        };
        Some(entry.meta.flag)
    }

    /// Syncs the active photograph's develop recipe into the catalog entry.
    pub fn update_active_recipe(&mut self, recipe: DevelopRecipe) {
        if let Some(entry) = self.active_entry_mut() {
            entry.meta.recipe = recipe;
        }
    }

    /// Copies a develop recipe onto the session adjustments clipboard.
    pub fn copy_adjustments(&mut self, recipe: &DevelopRecipe) {
        self.clipboard = Some(recipe.clone());
    }

    /// Copies the active photograph's develop recipe onto the session clipboard.
    pub fn copy_active_adjustments(&mut self) -> bool {
        let Some(recipe) = self.active_entry().map(|e| e.meta.recipe.clone()) else {
            return false;
        };
        self.clipboard = Some(recipe);
        true
    }

    /// Returns a cloned copy of the clipboard recipe if present.
    pub fn paste_adjustments(&self) -> Option<DevelopRecipe> {
        self.clipboard.clone()
    }

    /// Pastes the clipboard recipe into the active catalog entry and returns it.
    pub fn paste_to_active(&mut self) -> Option<DevelopRecipe> {
        let copied = self.clipboard.clone()?;
        let entry = self.active_entry_mut()?;
        entry.meta.recipe = copied.clone();
        Some(copied)
    }

    pub fn has_clipboard(&self) -> bool {
        self.clipboard.is_some()
    }

    /// Saves the `.rune-edit.json` sidecar for the entry at `index`.
    pub fn save_entry_sidecar(&self, index: usize) -> Result<Option<PathBuf>, CoreError> {
        let Some(entry) = self.entries.get(index) else {
            return Ok(None);
        };
        entry.meta.save_sidecar(&entry.path).map(Some)
    }

    /// Saves the `.rune-edit.json` sidecar for the currently active photograph.
    pub fn save_active_sidecar(&self) -> Result<Option<PathBuf>, CoreError> {
        let Some(idx) = self.active_index else {
            return Ok(None);
        };
        self.save_entry_sidecar(idx)
    }

    /// Returns `(total, picks, rejects)` counts across the catalog.
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut picks = 0usize;
        let mut rejects = 0usize;
        for entry in &self.entries {
            match entry.meta.flag {
                CullFlag::Pick => picks += 1,
                CullFlag::Reject => rejects += 1,
                CullFlag::None => {}
            }
        }
        (self.entries.len(), picks, rejects)
    }
}
