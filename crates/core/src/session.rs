//! Folder / Session Catalog, Culling, Rating, Copy/Paste Adjustments, and Sidecar Persistence.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{
    CoreError, DevelopRecipe,
    patch::{EditActor, EditHistoryEntry, FieldDiff},
};

/// Current `PhotoMeta` sidecar schema version (`3` adds `color_tag`, `active_variant_index`, and `variants`).
pub const META_SCHEMA_VERSION: u32 = 3;

/// Maximum provenance history entries retained in a `.rune-edit.json` sidecar.
pub const MAX_SIDECAR_HISTORY: usize = 64;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(1);

const fn default_meta_version() -> u32 {
    1
}

fn current_unix_timestamp_string() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    secs.to_string()
}

/// Culling pick/reject flag for a photograph in a session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CullFlag {
    #[default]
    None,
    Pick,
    Reject,
}

/// Capture One Pro 7-color semantic tag (`None` + 7 colors).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorTag {
    #[default]
    None,
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Purple,
    Pink,
}

impl ColorTag {
    pub const ALL: [Self; 8] = [
        Self::None,
        Self::Red,
        Self::Orange,
        Self::Yellow,
        Self::Green,
        Self::Blue,
        Self::Purple,
        Self::Pink,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Red => "Red",
            Self::Orange => "Orange",
            Self::Yellow => "Yellow",
            Self::Green => "Green",
            Self::Blue => "Blue",
            Self::Purple => "Purple",
            Self::Pink => "Pink",
        }
    }

    pub const fn cli_slug(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Red => "red",
            Self::Orange => "orange",
            Self::Yellow => "yellow",
            Self::Green => "green",
            Self::Blue => "blue",
            Self::Purple => "purple",
            Self::Pink => "pink",
        }
    }

    pub fn from_cli_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "clear" | "off" => Some(Self::None),
            "red" => Some(Self::Red),
            "orange" => Some(Self::Orange),
            "yellow" => Some(Self::Yellow),
            "green" => Some(Self::Green),
            "blue" => Some(Self::Blue),
            "purple" | "violet" => Some(Self::Purple),
            "pink" | "magenta" => Some(Self::Pink),
            _ => None,
        }
    }

    /// Returns crisp studio sRGB `[r, g, b]` tag swatch color.
    pub const fn srgb_u8(self) -> [u8; 3] {
        match self {
            Self::None => [120, 120, 124],
            Self::Red => [224, 72, 68],
            Self::Orange => [232, 132, 46],
            Self::Yellow => [228, 196, 56],
            Self::Green => [76, 184, 96],
            Self::Blue => [68, 138, 224],
            Self::Purple => [156, 102, 220],
            Self::Pink => [224, 96, 164],
        }
    }
}

/// Non-destructive Capture One-style virtual variant stored inside a `.rune-edit.json` sidecar.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PhotoVariant {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub recipe: DevelopRecipe,
    #[serde(default)]
    pub rating: u8,
    #[serde(default)]
    pub flag: CullFlag,
    #[serde(default)]
    pub color_tag: ColorTag,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

/// Per-photo catalog metadata persisted alongside the photo in `.rune-edit.json`.
///
/// Backward-compatible with v1/v2 `PhotoMeta` files and legacy bare `DevelopRecipe` files.
/// Top-level `recipe`, `rating`, `flag`, and `color_tag` always mirror the active variant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PhotoMeta {
    #[serde(default = "default_meta_version")]
    pub meta_version: u32,
    /// Monotonically incremented sidecar revision for optimistic concurrency checks.
    #[serde(default)]
    pub revision: u64,
    /// Star rating in `0..=5` (`0` = unrated).
    #[serde(default)]
    pub rating: u8,
    /// Culling pick/reject state.
    #[serde(default)]
    pub flag: CullFlag,
    /// Capture One 7-color semantic tag.
    #[serde(default)]
    pub color_tag: ColorTag,
    /// Non-destructive develop recipe for the active variant.
    #[serde(default)]
    pub recipe: DevelopRecipe,
    /// Index of the active virtual variant inside `variants`.
    #[serde(default)]
    pub active_variant_index: usize,
    /// Non-destructive virtual variants for this photograph (`>= 1` once materialized).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<PhotoVariant>,
    /// Persistent non-destructive provenance & 1-click revert log (capped at 64 entries).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<EditHistoryEntry>,
}

impl Default for PhotoMeta {
    fn default() -> Self {
        Self {
            meta_version: META_SCHEMA_VERSION,
            revision: 0,
            rating: 0,
            flag: CullFlag::None,
            color_tag: ColorTag::None,
            recipe: DevelopRecipe::default(),
            active_variant_index: 0,
            variants: Vec::new(),
            history: Vec::new(),
        }
    }
}

impl PhotoMeta {
    /// Materializes Variant 1 from top-level fields if `variants` is empty (e.g. v1/v2 sidecar)
    /// and clamps `active_variant_index` into bounds.
    pub fn ensure_variants(&mut self) {
        if self.variants.is_empty() {
            self.variants.push(PhotoVariant {
                id: "v1".to_owned(),
                label: "Variant 1".to_owned(),
                recipe: self.recipe.clone().sanitized(),
                rating: self.rating.min(5),
                flag: self.flag,
                color_tag: self.color_tag,
                created_at: Some(current_unix_timestamp_string()),
            });
            self.active_variant_index = 0;
        } else if self.active_variant_index >= self.variants.len() {
            self.active_variant_index = self.variants.len() - 1;
        }
    }

    /// Returns the logical number of virtual variants (`>= 1`).
    pub fn variant_count(&self) -> usize {
        self.variants.len().max(1)
    }

    /// Writes the active top-level fields (`recipe`, `rating`, `flag`, `color_tag`)
    /// back into `variants[active_variant_index]`.
    pub fn sync_active_to_variants(&mut self) {
        self.ensure_variants();
        self.rating = self.rating.min(5);
        self.recipe = self.recipe.clone().sanitized();
        if let Some(slot) = self.variants.get_mut(self.active_variant_index) {
            slot.recipe = self.recipe.clone();
            slot.rating = self.rating;
            slot.flag = self.flag;
            slot.color_tag = self.color_tag;
        }
    }

    fn next_variant_id_and_label(&self, label: Option<String>) -> (String, String) {
        let mut n = self.variants.len() + 1;
        loop {
            let candidate = format!("v{n}");
            if !self.variants.iter().any(|v| v.id == candidate) {
                let lbl = label
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| format!("Variant {n}"));
                return (candidate, lbl);
            }
            n += 1;
        }
    }

    /// Creates a fresh Neutral virtual variant, activates it, and returns its index.
    pub fn new_variant(&mut self, label: Option<String>) -> usize {
        self.sync_active_to_variants();
        let (id, lbl) = self.next_variant_id_and_label(label);
        let fresh = PhotoVariant {
            id,
            label: lbl,
            recipe: DevelopRecipe::default(),
            rating: 0,
            flag: CullFlag::None,
            color_tag: ColorTag::None,
            created_at: Some(current_unix_timestamp_string()),
        };
        self.variants.push(fresh);
        let idx = self.variants.len() - 1;
        self.active_variant_index = idx;
        self.recipe = DevelopRecipe::default();
        self.rating = 0;
        self.flag = CullFlag::None;
        self.color_tag = ColorTag::None;
        idx
    }

    /// Clones the currently active variant (recipe + rating + flag + color tag), activates it,
    /// and returns its index.
    pub fn clone_variant(&mut self, label: Option<String>) -> usize {
        self.sync_active_to_variants();
        let (id, lbl) = self.next_variant_id_and_label(label);
        let cloned = PhotoVariant {
            id,
            label: lbl,
            recipe: self.recipe.clone().sanitized(),
            rating: self.rating.min(5),
            flag: self.flag,
            color_tag: self.color_tag,
            created_at: Some(current_unix_timestamp_string()),
        };
        self.variants.push(cloned);
        let idx = self.variants.len() - 1;
        self.active_variant_index = idx;
        idx
    }

    /// Saves current top-level fields back to the old active slot and loads `variants[index]`
    /// into top-level `recipe`, `rating`, `flag`, and `color_tag`.
    pub fn select_variant(&mut self, index: usize) -> bool {
        self.sync_active_to_variants();
        if index >= self.variants.len() {
            return false;
        }
        self.active_variant_index = index;
        let chosen = self.variants[index].clone();
        self.recipe = chosen.recipe.sanitized();
        self.rating = chosen.rating.min(5);
        self.flag = chosen.flag;
        self.color_tag = chosen.color_tag;
        true
    }

    /// Deletes the variant at `index`, refusing when `variant_count() <= 1`.
    /// Updates `active_variant_index` and top-level fields cleanly.
    pub fn delete_variant(&mut self, index: usize) -> bool {
        self.sync_active_to_variants();
        if self.variants.len() <= 1 || index >= self.variants.len() {
            return false;
        }
        self.variants.remove(index);
        if index < self.active_variant_index {
            self.active_variant_index -= 1;
        } else if self.active_variant_index >= self.variants.len() {
            self.active_variant_index = self.variants.len() - 1;
        }
        let active = self.variants[self.active_variant_index].clone();
        self.recipe = active.recipe.sanitized();
        self.rating = active.rating.min(5);
        self.flag = active.flag;
        self.color_tag = active.color_tag;
        true
    }
    /// Appends a provenance entry to `self.history` (retaining entry #1 baseline + most recent 63).
    pub fn push_history(
        &mut self,
        actor: EditActor,
        summary: impl Into<String>,
        diffs: Vec<FieldDiff>,
        recipe_before: Option<DevelopRecipe>,
    ) {
        let next_id = self.history.last().map_or(1, |e| e.id + 1);
        let timestamp_unix_s = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        self.history.push(EditHistoryEntry {
            id: next_id,
            timestamp_unix_s,
            actor,
            summary: summary.into(),
            diffs,
            recipe_before,
        });
        if self.history.len() > MAX_SIDECAR_HISTORY {
            // Keep index 0 (initial baseline) and remove oldest intermediate entry.
            self.history.remove(1);
        }
    }

    /// Atomically serializes `PhotoMeta` to `.rune-edit.json` next to `photo_path`
    /// via a same-directory dot-prefixed temp file + `sync_all` + `rename`.
    ///
    /// Note: This immutable variant does not update `self.revision` in memory.
    /// Prefer [`Self::save_sidecar_in_place`] when retaining `PhotoMeta` across saves.
    pub fn save_sidecar(&self, photo_path: &Path) -> Result<PathBuf, CoreError> {
        self.save_sidecar_checked(photo_path, None)
    }

    /// Atomically serializes `PhotoMeta` and advances `self.revision` (plus sanitized fields)
    /// in place once the write succeeds.
    pub fn save_sidecar_in_place(&mut self, photo_path: &Path) -> Result<PathBuf, CoreError> {
        self.save_sidecar_checked_in_place(photo_path, None)
    }

    /// Atomically serializes `PhotoMeta`, optionally enforcing optimistic concurrency
    /// against `expected_revision` (failing with `CoreError::SidecarConflict` on mismatch).
    pub fn save_sidecar_checked(
        &self,
        photo_path: &Path,
        expected_revision: Option<u64>,
    ) -> Result<PathBuf, CoreError> {
        let mut clone = self.clone();
        clone.save_sidecar_checked_in_place(photo_path, expected_revision)
    }

    /// Atomically serializes `PhotoMeta` with optional optimistic concurrency checking,
    /// updating `self.revision`, `self.meta_version`, `self.rating`, and `self.recipe`
    /// in place only after the atomic disk write succeeds.
    pub fn save_sidecar_checked_in_place(
        &mut self,
        photo_path: &Path,
        expected_revision: Option<u64>,
    ) -> Result<PathBuf, CoreError> {
        let sidecar = DevelopRecipe::sidecar_path_for(photo_path);
        if let Some(expected) = expected_revision
            && let Some(existing) = Self::load_sidecar(photo_path)?
            && existing.revision != expected
        {
            return Err(CoreError::SidecarConflict {
                expected,
                actual: existing.revision,
            });
        }

        let mut to_write = self.clone();
        to_write.meta_version = META_SCHEMA_VERSION;
        to_write.revision = to_write.revision.saturating_add(1);
        to_write.rating = to_write.rating.min(5);
        to_write.recipe = to_write.recipe.sanitized();
        to_write.sync_active_to_variants();

        let json = serde_json::to_string_pretty(&to_write)?;
        atomic_write_bytes(&sidecar, json.as_bytes())?;

        self.meta_version = to_write.meta_version;
        self.revision = to_write.revision;
        self.rating = to_write.rating;
        self.color_tag = to_write.color_tag;
        self.recipe = to_write.recipe;
        self.active_variant_index = to_write.active_variant_index;
        self.variants = to_write.variants;
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

    /// Parses either a `PhotoMeta` JSON document (v1/v2/v3) or a legacy flat `DevelopRecipe` JSON document.
    pub fn from_sidecar_json(json: &str) -> Result<Self, CoreError> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        if value.get("recipe").is_some() {
            let mut meta: Self = serde_json::from_value(value)?;
            meta.rating = meta.rating.min(5);
            meta.recipe = meta.recipe.sanitized();
            if !meta.variants.is_empty() {
                if meta.active_variant_index >= meta.variants.len() {
                    meta.active_variant_index = meta.variants.len() - 1;
                }
                for v in &mut meta.variants {
                    v.rating = v.rating.min(5);
                    v.recipe = v.recipe.clone().sanitized();
                }
            } else {
                meta.active_variant_index = 0;
            }
            Ok(meta)
        } else {
            let recipe: DevelopRecipe = serde_json::from_value(value)?;
            Ok(Self {
                meta_version: 1,
                revision: 0,
                rating: 0,
                flag: CullFlag::None,
                color_tag: ColorTag::None,
                recipe: recipe.sanitized(),
                active_variant_index: 0,
                variants: Vec::new(),
                history: Vec::new(),
            })
        }
    }
}

/// Atomically writes `bytes` to `dest` via a hidden dot-prefixed temp file in the same
/// directory, calling `sync_all()` before `std::fs::rename`.
pub fn atomic_write_bytes(dest: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    let fname = dest
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sidecar.json".to_owned());
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(".{fname}.tmp.{}.{seq}", std::process::id()));

    let write_res = (|| -> std::io::Result<()> {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    })();

    if let Err(e) = write_res {
        let _ = std::fs::remove_file(&tmp);
        return Err(CoreError::Io(e));
    }

    if let Err(e) = std::fs::rename(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(CoreError::Io(e));
    }
    Ok(())
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionEntry {
    pub path: PathBuf,
    pub meta: PhotoMeta,
    #[serde(default)]
    pub color_tag: ColorTag,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens_label: Option<String>,
}

impl SessionEntry {
    pub fn new(path: PathBuf, meta: PhotoMeta) -> Self {
        let color_tag = meta.color_tag;
        Self {
            path,
            meta,
            color_tag,
            camera_label: None,
            lens_label: None,
        }
    }

    /// Returns the entry's effective color tag (synced with `meta.color_tag`).
    #[inline]
    pub fn effective_color_tag(&self) -> ColorTag {
        if self.meta.color_tag != ColorTag::None {
            self.meta.color_tag
        } else {
            self.color_tag
        }
    }
}

/// Folder / session catalog managing photo paths, active index, culling filters,
/// color tag & search filters, adjustment clipboard (Copy/Paste Adjustments), and sidecars.
#[derive(Clone, Debug, Default)]
pub struct SessionCatalog {
    pub folder: Option<PathBuf>,
    pub entries: Vec<SessionEntry>,
    pub active_index: Option<usize>,
    pub filter: CullFilter,
    pub color_tag_filter: Option<ColorTag>,
    pub search_query: String,
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
                SessionEntry::new(path, meta)
            })
            .collect();
        self.filter = CullFilter::All;
        self.color_tag_filter = None;
        self.search_query.clear();
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

    fn matches_filters(&self, entry: &SessionEntry, query_lower: &str) -> bool {
        if !self.filter.matches(&entry.meta) {
            return false;
        }
        if let Some(tag) = self.color_tag_filter
            && entry.effective_color_tag() != tag
        {
            return false;
        }
        if !query_lower.is_empty() {
            let fname_match = entry
                .path
                .file_name()
                .map(|s| {
                    s.to_string_lossy()
                        .to_ascii_lowercase()
                        .contains(query_lower)
                })
                .unwrap_or(false);
            let cam_match = entry
                .camera_label
                .as_deref()
                .is_some_and(|c| c.to_ascii_lowercase().contains(query_lower));
            let lens_match = entry
                .lens_label
                .as_deref()
                .is_some_and(|l| l.to_ascii_lowercase().contains(query_lower));
            let preset_match = entry
                .meta
                .recipe
                .preset_name
                .as_deref()
                .is_some_and(|p| p.to_ascii_lowercase().contains(query_lower));
            let tag_match = entry.effective_color_tag() != ColorTag::None
                && entry
                    .effective_color_tag()
                    .label()
                    .to_ascii_lowercase()
                    .contains(query_lower);
            if !(fname_match || cam_match || lens_match || preset_match || tag_match) {
                return false;
            }
        }
        true
    }

    /// Returns indices into `self.entries` that pass `filter`, `color_tag_filter`, and `search_query`.
    pub fn filtered_indices(&self) -> Vec<usize> {
        let query_lower = self.search_query.trim().to_ascii_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| self.matches_filters(entry, &query_lower).then_some(idx))
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

    fn snap_active_to_filtered(&mut self) {
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

    /// Updates the active filter and snaps `active_index` into the filtered view if needed.
    pub fn set_filter(&mut self, filter: CullFilter) {
        self.filter = filter;
        self.snap_active_to_filtered();
    }

    /// Updates the active color tag filter and snaps `active_index` into the filtered view if needed.
    pub fn set_color_tag_filter(&mut self, tag: Option<ColorTag>) {
        self.color_tag_filter = tag;
        self.snap_active_to_filtered();
    }

    /// Updates the case-insensitive filename/camera/lens/preset search query and snaps `active_index`.
    pub fn set_search_query(&mut self, query: impl Into<String>) {
        self.search_query = query.into();
        self.snap_active_to_filtered();
    }

    /// Attaches cached EXIF camera/lens display strings onto an entry for faceted search filtering.
    pub fn set_entry_exif_labels(
        &mut self,
        index: usize,
        camera_label: Option<String>,
        lens_label: Option<String>,
    ) -> bool {
        let Some(entry) = self.entries.get_mut(index) else {
            return false;
        };
        entry.camera_label = camera_label;
        entry.lens_label = lens_label;
        true
    }

    /// Sets the Capture One color tag on entry `index` (syncing both `entry.color_tag` and `entry.meta.color_tag`).
    pub fn set_color_tag(&mut self, index: usize, tag: ColorTag) -> bool {
        let Some(entry) = self.entries.get_mut(index) else {
            return false;
        };
        entry.color_tag = tag;
        entry.meta.color_tag = tag;
        true
    }

    /// Sets the Capture One color tag on the active photo.
    pub fn set_active_color_tag(&mut self, tag: ColorTag) -> bool {
        let Some(idx) = self.active_index else {
            return false;
        };
        self.set_color_tag(idx, tag)
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

    /// Saves the `.rune-edit.json` sidecar for the entry at `index`, advancing its
    /// in-memory `meta.revision` on success.
    pub fn save_entry_sidecar_mut(&mut self, index: usize) -> Result<Option<PathBuf>, CoreError> {
        let Some(entry) = self.entries.get_mut(index) else {
            return Ok(None);
        };
        entry.meta.save_sidecar_in_place(&entry.path).map(Some)
    }

    /// Saves the `.rune-edit.json` sidecar for the currently active photograph.
    pub fn save_active_sidecar(&self) -> Result<Option<PathBuf>, CoreError> {
        let Some(idx) = self.active_index else {
            return Ok(None);
        };
        self.save_entry_sidecar(idx)
    }

    /// Saves the `.rune-edit.json` sidecar for the currently active photograph,
    /// advancing its in-memory `meta.revision` on success.
    pub fn save_active_sidecar_mut(&mut self) -> Result<Option<PathBuf>, CoreError> {
        let Some(idx) = self.active_index else {
            return Ok(None);
        };
        self.save_entry_sidecar_mut(idx)
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

/// Capture One Pro 4-folder Session directory layout (`Capture`, `Selects`, `Output`, `Trash`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFolderLayout {
    pub root: PathBuf,
    pub capture: PathBuf,
    pub selects: PathBuf,
    pub output: PathBuf,
    pub trash: PathBuf,
}

/// Idempotently creates a Capture One-style 4-folder session (`Capture/`, `Selects/`, `Output/`,
/// `Trash/`) under `root`.
pub fn create_capture_one_session(root: &Path) -> Result<SessionFolderLayout, CoreError> {
    if root.as_os_str().is_empty() {
        return Err(CoreError::Validation(
            "Session root path cannot be empty".to_owned(),
        ));
    }
    let capture = root.join("Capture");
    let selects = root.join("Selects");
    let output = root.join("Output");
    let trash = root.join("Trash");
    for dir in [root, &capture, &selects, &output, &trash] {
        std::fs::create_dir_all(dir)?;
    }
    Ok(SessionFolderLayout {
        root: root.to_path_buf(),
        capture,
        selects,
        output,
        trash,
    })
}

/// Safely moves `photo` (and its sibling `.rune-edit.json` sidecar if present) into `dest_dir`
/// (e.g. `Selects/` or `Trash/`), strictly refusing to clobber any existing file or symlink in
/// `dest_dir`.
pub fn move_photo_with_sidecar(photo: &Path, dest_dir: &Path) -> Result<PathBuf, CoreError> {
    let meta = std::fs::symlink_metadata(photo).map_err(|e| {
        CoreError::Validation(format!(
            "Cannot move '{}': source photo unavailable ({e})",
            photo.display()
        ))
    })?;
    if !meta.file_type().is_file() {
        return Err(CoreError::Validation(format!(
            "Cannot move '{}': source must be a regular file",
            photo.display()
        )));
    }
    let fname = photo.file_name().ok_or_else(|| {
        CoreError::Validation(format!(
            "Cannot move '{}': missing filename",
            photo.display()
        ))
    })?;

    std::fs::create_dir_all(dest_dir)?;
    let dest_photo = dest_dir.join(fname);

    // Refuse no-op move onto self or clobbering any existing file/symlink at destination.
    if dest_photo.exists() || dest_photo.symlink_metadata().is_ok() {
        return Err(CoreError::Validation(format!(
            "Refusing to overwrite existing destination file '{}'",
            dest_photo.display()
        )));
    }

    let src_sidecar = DevelopRecipe::sidecar_path_for(photo);
    let has_sidecar = src_sidecar.is_file();
    let dest_sidecar = DevelopRecipe::sidecar_path_for(&dest_photo);
    if has_sidecar && (dest_sidecar.exists() || dest_sidecar.symlink_metadata().is_ok()) {
        return Err(CoreError::Validation(format!(
            "Refusing to overwrite existing destination sidecar '{}'",
            dest_sidecar.display()
        )));
    }

    move_file_safe(photo, &dest_photo)?;
    if has_sidecar && let Err(e) = move_file_safe(&src_sidecar, &dest_sidecar) {
        // Best-effort rollback of photo move if sidecar move fails.
        let _ = move_file_safe(&dest_photo, photo);
        return Err(e);
    }

    Ok(dest_photo)
}

fn move_file_safe(src: &Path, dst: &Path) -> Result<(), CoreError> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(rename_err) => {
            // Cross-device move fallback: copy to temp dotfile in dest dir, sync, rename, then remove src.
            let bytes = std::fs::read(src).map_err(|_| CoreError::Io(rename_err))?;
            atomic_write_bytes(dst, &bytes)?;
            std::fs::remove_file(src)?;
            Ok(())
        }
    }
}
