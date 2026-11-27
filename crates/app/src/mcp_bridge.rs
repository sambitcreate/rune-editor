//! Live GUI ↔ Local MCP Server bridge.
//!
//! Synchronizes a lock-protected snapshot of the active darkroom canvas so MCP HTTP
//! worker threads can read image/recipe state without blocking the 120Hz UI thread,
//! and routes agent edits back into the UI via `AppHandle::update_if` with full `⌘Z`
//! undo support.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rune::AppHandle;
use rune_editor_core::{ColorTag, CullFlag, DevelopRecipe, PhotoMeta};
use rune_editor_mcp::{ActiveCanvasSnapshot, McpHostBridge};

use crate::state::AppState;

/// Host bridge connecting `rune_editor_mcp::LocalMcpServer` to the live `AppState`.
pub struct GuiMcpBridge {
    pub handle: AppHandle<AppState>,
    pub shared_canvas: Arc<Mutex<ActiveCanvasSnapshot>>,
    pub allow_live_edits: Arc<AtomicBool>,
}

impl McpHostBridge for GuiMcpBridge {
    fn active_canvas(&self) -> Option<ActiveCanvasSnapshot> {
        self.shared_canvas.lock().ok().map(|g| g.clone())
    }

    fn allows_live_edits(&self) -> bool {
        self.allow_live_edits.load(Ordering::Relaxed)
    }

    fn apply_live_recipe(
        &self,
        target_path: Option<&Path>,
        new_recipe: DevelopRecipe,
        summary: String,
    ) -> Result<bool, String> {
        if !self.allow_live_edits.load(Ordering::Relaxed) {
            return Err(
                "Live canvas edits are disabled in Rune Darkroom Settings → Agent & MCP."
                    .to_owned(),
            );
        }

        let target_owned = target_path.map(Path::to_path_buf);
        let shared = self.shared_canvas.clone();
        if let Ok(mut guard) = shared.lock() {
            let matches = match (&target_owned, &guard.source_path) {
                (None, _) => true,
                (Some(tp), Some(sp)) => tp == sp,
                (Some(_), None) => false,
            };
            if !matches {
                return Ok(false);
            }
            guard.recipe = new_recipe.clone();
        }

        self.handle.update_if(move |s, tx| {
            let matches = match (&target_owned, &s.image.source_path) {
                (None, _) => true,
                (Some(tp), Some(sp)) => tp == sp,
                (Some(_), None) => false,
            };
            if !matches {
                return false;
            }
            s.apply_external_mcp_recipe_tx(new_recipe, summary, tx);
            true
        });

        Ok(true)
    }

    fn apply_live_cull(
        &self,
        target_path: Option<&Path>,
        rating: Option<u8>,
        flag: Option<CullFlag>,
        color_tag: Option<ColorTag>,
    ) -> Result<bool, String> {
        let target_owned = target_path.map(Path::to_path_buf);
        if let Ok(mut guard) = self.shared_canvas.lock() {
            let matches = match (&target_owned, &guard.source_path) {
                (None, _) => true,
                (Some(tp), Some(sp)) => tp == sp,
                (Some(_), None) => false,
            };
            if !matches {
                return Ok(false);
            }
            if let Some(r) = rating {
                guard.rating = r.min(5);
            }
            if let Some(f) = flag {
                guard.flag = f;
            }
            if let Some(tag) = color_tag {
                guard.color_tag = tag;
            }
        }

        self.handle.update_if(move |s, tx| {
            let matches = match (&target_owned, &s.image.source_path) {
                (None, _) => true,
                (Some(tp), Some(sp)) => tp == sp,
                (Some(_), None) => false,
            };
            if !matches {
                return false;
            }
            if target_owned.is_some() {
                // MCP already persisted the sidecar for the explicit path; update in-memory
                // state and reload revision/history/variants/color_tag without double-writing.
                if let Some(r) = rating {
                    s.catalog.set_active_rating(r);
                }
                if let Some(f) = flag {
                    s.catalog.set_active_flag(f);
                }
                if let Some(tag) = color_tag {
                    s.catalog.set_active_color_tag(tag);
                }
                if let Some(path) = s.image.source_path.as_deref()
                    && let Ok(Some(disk_meta)) = PhotoMeta::load_sidecar(path)
                    && let Some(entry) = s.catalog.active_entry_mut()
                {
                    entry.meta.revision = disk_meta.revision;
                    entry.meta.history = disk_meta.history;
                    entry.meta.rating = disk_meta.rating;
                    entry.meta.flag = disk_meta.flag;
                    entry.meta.color_tag = disk_meta.color_tag;
                    entry.color_tag = disk_meta.color_tag;
                    entry.meta.active_variant_index = disk_meta.active_variant_index;
                    entry.meta.variants = disk_meta.variants;
                }
                s.sync_window_state(tx);
            } else {
                let mut mutated = false;
                if let Some(r) = rating {
                    mutated |= s.catalog.set_active_rating(r);
                }
                if let Some(f) = flag {
                    mutated |= s.catalog.set_active_flag(f);
                }
                if let Some(tag) = color_tag {
                    mutated |= s.catalog.set_active_color_tag(tag);
                }
                if mutated {
                    if !s.demo_mode
                        && let Some(idx) = s.catalog.active_index
                    {
                        let _ = s.catalog.save_entry_sidecar_mut(idx);
                    }
                    s.sync_window_state(tx);
                }
            }
            true
        });

        Ok(true)
    }

    fn on_activity(&self, tool_name: &str, ok: bool) {
        let tool = tool_name.to_owned();
        self.handle.update_if(move |s, _| {
            s.mcp_request_count = s.mcp_request_count.saturating_add(1);
            s.mcp_last_activity = Some(format!("{tool} • {}", if ok { "ok" } else { "error" }));
            true
        });
    }
}
