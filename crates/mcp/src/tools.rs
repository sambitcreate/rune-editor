//! Shared 9-tool MCP dispatcher, warm RAW proxy cache, MCP Resources, and
//! live GUI Host Bridge trait (`McpHostBridge`).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rune_editor_core::{
    ADVANCED_COLOR_SLICE_COUNT, AdjustmentLayer, BUILTIN_CAMERA_PROFILES, BrushDab, ColorTag,
    CullFlag, DevelopRecipe, EditActor, FilmCurve, HealMode, HealSpot, LayerMaskItem,
    LocalAdjustments, MAX_ADJUSTMENT_LAYERS, MAX_BRUSH_DABS_PER_MASK, MAX_HEAL_SPOTS,
    MAX_MASKS_PER_LAYER, MaskCombineOp, MaskShape, PhotoMeta, ProcessingMode, ProfileCategory,
    RecipePatch, UserStyle, VignetteMethod, WbPreset, all_param_descriptors, analyze_linear_proxy,
    apply_style_to_recipe, auto_exposure_and_tone, auto_levels, auto_white_balance, blend_recipes,
    builtin_presets, canonical_profile_id, compute_histogram, copy_recipe_groups,
    create_capture_one_session, default_profiles_dir, default_styles_dir, delete_user_style_in_dir,
    diff_recipes, is_valid_camera_profile_id, list_all_camera_profiles, list_user_styles_in_dir,
    load_user_style_in_dir, modified_recipe_groups, pick_white_balance, recipe_field_diffs,
    recipe_non_default_map, recipe_schema_json, reset_recipe_group, resolve_builtin_preset,
    sanitize_style_slug, save_user_style_in_dir, set_param_by_str, stack_style_onto_recipe,
    temperature_to_kelvin,
};
use rune_editor_decode::{
    ExportColorSpace, ExportFormat, ExportOptions, ExportRecipeSpec, LinearImage, NamingContext,
    TextWatermark, WatermarkPosition, builtin_export_recipes, expand_naming_template,
    export_image_to_jpeg_vec, export_image_with_options, export_multi_recipe,
    extract_detail_luma_proxy, extract_exif_from_path, generate_subject_mask, guard_not_source,
    is_raw_extension, load_file, load_file_proxy, plan_multi_recipe_paths, sample_linear_rgb_at_uv,
    sample_raw_linear_rgb_at_uv,
};
use serde_json::{Value, json};

use crate::protocol::{
    JsonRpcRequest, ToolAnnotations, ToolDef, jsonrpc_error, jsonrpc_ok,
    negotiate_protocol_version, rpc_err, tool_err, tool_ok,
};

/// Image extensions permitted for MCP file outputs (`out` on `rune_preview_photo` and `rune_cull_or_export`).
const ALLOWED_MCP_OUTPUT_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "webp"];

/// Snapshot of the currently open photograph inside the Rune Darkroom desktop GUI.
#[derive(Clone, Debug)]
pub struct ActiveCanvasSnapshot {
    pub display_name: String,
    pub source_path: Option<PathBuf>,
    pub image: Arc<LinearImage>,
    pub recipe: DevelopRecipe,
    pub rating: u8,
    pub flag: CullFlag,
    pub color_tag: ColorTag,
    pub demo_mode: bool,
    pub folder: Option<PathBuf>,
    pub catalog_len: usize,
}

/// Bridge allowing the MCP server to inspect and mutate the live Rune Darkroom GUI
/// session on the user's behalf (or operate headlessly in `rd mcp` stdio mode).
pub trait McpHostBridge: Send + Sync {
    /// Returns the currently active GUI photo/recipe if running inside Rune Darkroom.
    fn active_canvas(&self) -> Option<ActiveCanvasSnapshot> {
        None
    }

    /// Applies an updated recipe directly to the live GUI canvas (pushing undo history so
    /// the human can hit `⌘Z` to revert!). Returns `true` if applied to the active GUI canvas.
    fn apply_live_recipe(
        &self,
        _target_path: Option<&Path>,
        _new_recipe: DevelopRecipe,
        _summary: String,
    ) -> Result<bool, String> {
        Ok(false)
    }

    /// Updates rating, cull flag, and color tag on the live GUI session when applicable.
    fn apply_live_cull(
        &self,
        _target_path: Option<&Path>,
        _rating: Option<u8>,
        _flag: Option<CullFlag>,
        _color_tag: Option<ColorTag>,
    ) -> Result<bool, String> {
        Ok(false)
    }

    /// Notifies the host (e.g., Settings UI) whenever an agent invokes an MCP tool.
    fn on_activity(&self, _tool_name: &str, _ok: bool) {}
}

/// Default headless bridge used by `rd mcp` CLI (operates strictly on photo paths on disk).
pub struct HeadlessHostBridge;
impl McpHostBridge for HeadlessHostBridge {}

/// Thread-safe LRU cache of decoded proxy `LinearImage`s (`<= 4` entries) so iterative
/// `analyze` -> `edit` -> `preview` agent turns execute in milliseconds.
struct ProxyCache {
    entries: VecDeque<(PathBuf, u32, Arc<LinearImage>)>,
}

impl ProxyCache {
    fn new() -> Self {
        Self {
            entries: VecDeque::with_capacity(4),
        }
    }

    fn get_or_load(&mut self, path: &Path, max_edge: u32) -> Result<Arc<LinearImage>, String> {
        if let Some(pos) = self
            .entries
            .iter()
            .position(|(p, edge, _)| p == path && *edge == max_edge)
        {
            let entry = self.entries.remove(pos).unwrap();
            let img = entry.2.clone();
            self.entries.push_front(entry);
            return Ok(img);
        }
        let img = Arc::new(
            load_file_proxy(path, Some(max_edge)).map_err(|e| format!("Decode failed: {e}"))?,
        );
        if self.entries.len() >= 4 {
            self.entries.pop_back();
        }
        self.entries
            .push_front((path.to_path_buf(), max_edge, img.clone()));
        Ok(img)
    }
}

/// Core transport-agnostic MCP 2025-11-25 service shared by Streamable HTTP and stdio.
pub struct McpService {
    host: Arc<dyn McpHostBridge>,
    cache: Mutex<ProxyCache>,
    request_count: AtomicU64,
    last_tool: Mutex<Option<String>>,
    styles_dir_override: Option<PathBuf>,
    profiles_dir_override: Option<PathBuf>,
}

impl McpService {
    pub fn new(host: Arc<dyn McpHostBridge>) -> Self {
        Self {
            host,
            cache: Mutex::new(ProxyCache::new()),
            request_count: AtomicU64::new(0),
            last_tool: Mutex::new(None),
            styles_dir_override: None,
            profiles_dir_override: None,
        }
    }

    /// Configures a custom User Styles directory (Rust-only API for hermetic tests; never
    /// exposed to untrusted MCP JSON-RPC arguments).
    pub fn with_styles_dir(mut self, dir: PathBuf) -> Self {
        self.styles_dir_override = Some(dir);
        self
    }

    /// Configures a custom Camera Profiles directory (Rust-only API for hermetic tests; never
    /// exposed to untrusted MCP JSON-RPC arguments).
    pub fn with_profiles_dir(mut self, dir: PathBuf) -> Self {
        self.profiles_dir_override = Some(dir);
        self
    }

    pub fn request_count(&self) -> u64 {
        self.request_count.load(Ordering::Relaxed)
    }

    pub fn last_tool(&self) -> Option<String> {
        self.last_tool.lock().ok().and_then(|g| g.clone())
    }

    /// Handles a single JSON-RPC 2.0 request. Returns `None` for valid notifications (no `id`).
    pub fn handle_request(&self, req: &JsonRpcRequest) -> Option<Value> {
        if !req.is_valid_jsonrpc() {
            return Some(jsonrpc_error(
                req.id.clone(),
                rpc_err::INVALID_REQUEST,
                format!(
                    "Invalid JSON-RPC version '{}': expected \"2.0\".",
                    req.jsonrpc
                ),
            ));
        }
        if req.is_notification() {
            return None;
        }
        let id = req.id.clone().unwrap_or(Value::Null);
        let params = req.params.clone().unwrap_or_else(|| json!({}));

        let resp = match req.method.as_str() {
            "initialize" => self.handle_initialize(id, &params),
            "ping" => jsonrpc_ok(id, json!({})),
            "tools/list" => jsonrpc_ok(id, json!({ "tools": tool_definitions() })),
            "tools/call" => self.handle_tools_call(id, &params),
            "resources/list" => jsonrpc_ok(
                id,
                json!({
                    "resources": [
                        {
                            "uri": "rune://schema/recipe",
                            "name": "Develop Recipe Schema",
                            "description": format!(
                                "Live single-source registry of all {} Rune Darkroom parameters, bounds, tool groups, and built-in presets.",
                                all_param_descriptors().len()
                            ),
                            "mimeType": "application/json"
                        },
                        {
                            "uri": "rune://session/active",
                            "name": "Active Rune Darkroom Session",
                            "description": "Current open photograph, develop recipe overrides, rating, and folder session state when connected to the desktop app.",
                            "mimeType": "application/json"
                        }
                    ]
                }),
            ),
            "resources/read" => self.handle_resources_read(id, &params),
            other => jsonrpc_error(
                Some(id),
                rpc_err::METHOD_NOT_FOUND,
                format!("Method not found: {other}"),
            ),
        };
        Some(resp)
    }

    fn handle_initialize(&self, id: Value, params: &Value) -> Value {
        let req_ver = params.get("protocolVersion").and_then(Value::as_str);
        let negotiated = negotiate_protocol_version(req_ver);
        jsonrpc_ok(
            id,
            json!({
                "protocolVersion": negotiated,
                "capabilities": {
                    "tools": { "listChanged": false },
                    "resources": { "subscribe": false, "listChanged": false }
                },
                "serverInfo": {
                    "name": "rune-darkroom",
                    "title": "Rune Darkroom Local MCP",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "instructions": "Non-destructive RAW & photo editing MCP server for Rune Darkroom. Call `rune_get_schema` for exact parameter bounds, `rune_analyze_photo` for tonal/color/skin/sharpness metrics, `rune_edit_photo` to update edits non-destructively (updates the live GUI canvas with Cmd+Z undo when connected), and `rune_preview_photo` to visually inspect edits via inline JPEG."
            }),
        )
    }

    fn handle_resources_read(&self, id: Value, params: &Value) -> Value {
        let Some(uri) = params.get("uri").and_then(Value::as_str) else {
            return jsonrpc_error(Some(id), rpc_err::INVALID_PARAMS, "Missing resource `uri`");
        };
        match uri {
            "rune://schema/recipe" => {
                let text = serde_json::to_string_pretty(&recipe_schema_json()).unwrap_or_default();
                jsonrpc_ok(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": text
                        }]
                    }),
                )
            }
            "rune://session/active" => {
                let payload = self.active_session_json();
                let text = serde_json::to_string_pretty(&payload).unwrap_or_default();
                jsonrpc_ok(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": text
                        }]
                    }),
                )
            }
            _ => jsonrpc_error(
                Some(id),
                rpc_err::INVALID_PARAMS,
                format!("Unknown resource URI: {uri}"),
            ),
        }
    }

    fn handle_tools_call(&self, id: Value, params: &Value) -> Value {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return jsonrpc_error(Some(id), rpc_err::INVALID_PARAMS, "Missing tool `name`");
        };
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        self.request_count.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut g) = self.last_tool.lock() {
            *g = Some(name.to_owned());
        }

        let result = match name {
            "rune_get_schema" => self.tool_get_schema(),
            "rune_inspect_photo" => self.tool_inspect_photo(&args),
            "rune_analyze_photo" => self.tool_analyze_photo(&args),
            "rune_edit_photo" => self.tool_edit_photo(&args),
            "rune_preview_photo" => self.tool_preview_photo(&args),
            "rune_diff_photo" => self.tool_diff_photo(&args),
            "rune_cull_or_export" => self.tool_cull_or_export(&args),
            "rune_manage_workflow" => self.tool_manage_workflow(&args),
            "rune_edit_layers" => self.tool_edit_layers(&args),
            _ => {
                self.host.on_activity(name, false);
                return jsonrpc_error(
                    Some(id),
                    rpc_err::INVALID_PARAMS,
                    format!("Unknown tool: {name}"),
                );
            }
        };

        let ok = !result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.host.on_activity(name, ok);
        jsonrpc_ok(id, result)
    }

    fn active_session_json(&self) -> Value {
        if let Some(snap) = self.host.active_canvas() {
            json!({
                "connected_to_gui": true,
                "display_name": snap.display_name,
                "source_path": snap.source_path,
                "dimensions": [snap.image.width, snap.image.height],
                "demo_mode": snap.demo_mode,
                "folder": snap.folder,
                "catalog_len": snap.catalog_len,
                "rating": snap.rating,
                "flag": snap.flag,
                "color_tag": snap.color_tag,
                "wb_kelvin": temperature_to_kelvin(snap.recipe.temperature),
                "wb_preset": snap.recipe.wb_preset,
                "camera_profile": snap.recipe.camera_profile,
                "profile_strength": snap.recipe.profile_strength,
                "film_curve": snap.recipe.film_curve,
                "processing_mode": snap.recipe.processing_mode,
                "recipe_overrides": recipe_non_default_map(&snap.recipe),
            })
        } else {
            json!({
                "connected_to_gui": false,
                "note": "Running in headless stdio mode; pass explicit `path` to photo tools."
            })
        }
    }

    fn resolve_target(
        &self,
        args: &Value,
        proxy_edge: u32,
    ) -> Result<(Option<PathBuf>, Arc<LinearImage>, PhotoMeta, bool), Value> {
        let explicit = args
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);

        if let Some(path) = explicit {
            if !path.exists() {
                return Err(tool_err(
                    "INPUT_NOT_FOUND",
                    format!("Photo does not exist: {}", path.display()),
                    Some("Pass a valid file path or omit `path` to target the active GUI canvas."),
                ));
            }
            let meta = PhotoMeta::load_sidecar(&path)
                .map_err(|e| tool_err("IO_ERROR", e.to_string(), None))?
                .unwrap_or_default();
            let img = self
                .cache
                .lock()
                .map_err(|_| tool_err("INTERNAL_ERROR", "Cache lock poisoned", None))?
                .get_or_load(&path, proxy_edge)
                .map_err(|e| tool_err("DECODE_FAILED", e, None))?;
            let is_active = self
                .host
                .active_canvas()
                .and_then(|s| s.source_path)
                .is_some_and(|active_p| active_p == path);
            Ok((Some(path), img, meta, is_active))
        } else if let Some(snap) = self.host.active_canvas() {
            // Preserve existing on-disk sidecar provenance (`revision` + `history`) when
            // targeting a live GUI canvas backed by a real photo file.
            let mut meta = match snap.source_path.as_deref() {
                Some(p) => PhotoMeta::load_sidecar(p)
                    .map_err(|e| tool_err("IO_ERROR", e.to_string(), None))?
                    .unwrap_or_default(),
                None => PhotoMeta::default(),
            };
            meta.rating = snap.rating;
            meta.flag = snap.flag;
            meta.recipe = snap.recipe;
            Ok((snap.source_path, snap.image, meta, true))
        } else {
            Err(tool_err(
                "MISSING_PATH",
                "No `path` provided and no active GUI photo canvas is connected.",
                Some("Pass `path` pointing to a RAW/JPEG/PNG/TIFF/HEIC photo on disk."),
            ))
        }
    }

    fn tool_get_schema(&self) -> Value {
        let data = json!({
            "schema": recipe_schema_json(),
            "active_session": self.active_session_json(),
        });
        tool_ok(data, None, None)
    }

    fn tool_inspect_photo(&self, args: &Value) -> Value {
        let explicit = args
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);

        if let Some(path) = explicit {
            if !path.exists() {
                return tool_err(
                    "INPUT_NOT_FOUND",
                    format!("Photo does not exist: {}", path.display()),
                    None,
                );
            }
            let loaded = match PhotoMeta::load_sidecar(&path) {
                Ok(m) => m,
                Err(e) => return tool_err("IO_ERROR", e.to_string(), None),
            };
            let has_sidecar = loaded.is_some();
            let meta = loaded.unwrap_or_default();
            let exif = extract_exif_from_path(&path);
            let mut data = json!({
                "path": path,
                "is_raw": is_raw_extension(&path),
                "has_sidecar": has_sidecar,
                "rating": meta.rating,
                "flag": meta.flag,
                "color_tag": meta.color_tag,
                "active_variant_index": meta.active_variant_index,
                "variant_count": meta.variant_count(),
                "variants": variants_summary_json(&meta),
                "layer_count": meta.recipe.layers.len(),
                "layers": layers_summary_json(&meta.recipe.layers),
                "heal_spot_count": meta.recipe.heal_spots.len(),
                "revision": meta.revision,
                "history_count": meta.history.len(),
                "exif": exif,
                "wb_kelvin": temperature_to_kelvin(meta.recipe.temperature),
                "wb_preset": meta.recipe.wb_preset,
                "camera_profile": meta.recipe.camera_profile,
                "profile_strength": meta.recipe.profile_strength,
                "film_curve": meta.recipe.film_curve,
                "processing_mode": meta.recipe.processing_mode,
                "recipe_overrides": recipe_non_default_map(&meta.recipe),
            });
            if (meta.recipe.processing_mode == ProcessingMode::FilmNegative
                || !meta.recipe.film_negative.is_default())
                && let Some(obj) = data.as_object_mut()
            {
                obj.insert("film_negative".to_owned(), json!(meta.recipe.film_negative));
            }
            tool_ok(data, None, None)
        } else if let Some(snap) = self.host.active_canvas() {
            let disk_meta = snap
                .source_path
                .as_deref()
                .and_then(|p| PhotoMeta::load_sidecar(p).ok().flatten());
            let mut effective_meta = disk_meta.clone().unwrap_or_else(|| PhotoMeta {
                rating: snap.rating,
                flag: snap.flag,
                color_tag: snap.color_tag,
                recipe: snap.recipe.clone(),
                ..PhotoMeta::default()
            });
            effective_meta.color_tag = snap.color_tag;
            let mut data = json!({
                "display_name": snap.display_name,
                "path": snap.source_path,
                "dimensions": [snap.image.width, snap.image.height],
                "demo_mode": snap.demo_mode,
                "has_sidecar": disk_meta.is_some(),
                "revision": disk_meta.as_ref().map_or(0, |m| m.revision),
                "history_count": disk_meta.as_ref().map_or(0, |m| m.history.len()),
                "rating": snap.rating,
                "flag": snap.flag,
                "color_tag": snap.color_tag,
                "active_variant_index": effective_meta.active_variant_index,
                "variant_count": effective_meta.variant_count(),
                "variants": variants_summary_json(&effective_meta),
                "layer_count": snap.recipe.layers.len(),
                "layers": layers_summary_json(&snap.recipe.layers),
                "heal_spot_count": snap.recipe.heal_spots.len(),
                "exif": snap.image.exif,
                "wb_kelvin": temperature_to_kelvin(snap.recipe.temperature),
                "wb_preset": snap.recipe.wb_preset,
                "camera_profile": snap.recipe.camera_profile,
                "profile_strength": snap.recipe.profile_strength,
                "film_curve": snap.recipe.film_curve,
                "processing_mode": snap.recipe.processing_mode,
                "recipe_overrides": recipe_non_default_map(&snap.recipe),
            });
            if (snap.recipe.processing_mode == ProcessingMode::FilmNegative
                || !snap.recipe.film_negative.is_default())
                && let Some(obj) = data.as_object_mut()
            {
                obj.insert("film_negative".to_owned(), json!(snap.recipe.film_negative));
            }
            tool_ok(data, None, None)
        } else {
            tool_err(
                "MISSING_PATH",
                "Provide `path` or connect to an open Rune Darkroom window.",
                None,
            )
        }
    }

    fn tool_analyze_photo(&self, args: &Value) -> Value {
        let proxy_edge = args
            .get("proxy_edge")
            .and_then(Value::as_u64)
            .unwrap_or(320)
            .clamp(64, 1600) as u32;
        let fast = args.get("fast").and_then(Value::as_bool).unwrap_or(false);

        let (path_opt, img, meta, is_active) = match self.resolve_target(args, proxy_edge) {
            Ok(v) => v,
            Err(e) => return e,
        };

        let detail = if !fast {
            path_opt
                .as_deref()
                .and_then(|p| extract_detail_luma_proxy(p, 1024).ok())
        } else {
            None
        };
        let detail_ref = detail.as_ref().map(|(b, w, h)| (b.as_slice(), *w, *h));
        let analysis = analyze_linear_proxy(&img.histogram_proxy, &meta.recipe, detail_ref);

        let data = json!({
            "path": path_opt,
            "active_gui_canvas": is_active,
            "dimensions": [img.width, img.height],
            "exif": img.exif,
            "rating": meta.rating,
            "flag": meta.flag,
            "recipe_overrides": recipe_non_default_map(&meta.recipe),
            "analysis": analysis,
        });
        tool_ok(data, None, None)
    }

    fn tool_edit_photo(&self, args: &Value) -> Value {
        if let Some(obj) = args.as_object() {
            const ALLOWED: &[&str] = &[
                "path",
                "preset",
                "mix",
                "auto",
                "camera_profile",
                "profile_strength",
                "wb_pick",
                "wb_preset",
                "film_curve",
                "processing_mode",
                "film_base_pick",
                "vignette_method",
                "skin_pick",
                "color_slice_pick",
                "patch",
                "set",
                "reset",
                "agent",
                "dry_run",
            ];
            for key in obj.keys() {
                if !ALLOWED.contains(&key.as_str()) {
                    return tool_err(
                        "UNKNOWN_ARGUMENT",
                        format!("Unknown top-level argument '{key}' on rune_edit_photo."),
                        Some(
                            "Place develop parameters inside `patch` (e.g. `{\"patch\":{\"lens\":{\"distortion\":-0.15}}}`) or `set` (e.g. `{\"set\":{\"lens.distortion\":-0.15}}`). Top-level shortcuts: `preset`, `camera_profile`, `profile_strength`, `wb_preset`, `film_curve`, `processing_mode`, `film_base_pick`, `vignette_method`, `wb_pick`, `skin_pick`, `color_slice_pick`, `auto`, `reset`.",
                        ),
                    );
                }
            }
        }

        let camera_profile_arg = match args.get("camera_profile") {
            None => None,
            Some(v) => match v.as_str() {
                Some(s) => {
                    if !is_valid_camera_profile_id(s) {
                        let valid = BUILTIN_CAMERA_PROFILES
                            .iter()
                            .map(|p| p.id)
                            .collect::<Vec<_>>()
                            .join(", ");
                        return tool_err(
                            "VALIDATION_ERROR",
                            format!("Invalid `camera_profile` '{s}'."),
                            Some(&format!(
                                "Pass a bundled profile ID ({valid}) or `user:<slug>`. Call `rune_manage_workflow` with `action: \"profile_list\"` to inspect installed profiles."
                            )),
                        );
                    }
                    Some(canonical_profile_id(s))
                }
                None => {
                    return tool_err(
                        "VALIDATION_ERROR",
                        "`camera_profile` must be a string.",
                        Some(
                            "Call `rune_manage_workflow` with `action: \"profile_list\"` to inspect available profile IDs.",
                        ),
                    );
                }
            },
        };
        let profile_strength_arg = match args.get("profile_strength") {
            None => None,
            Some(v) => match v.as_f64() {
                Some(n) => {
                    let f = n as f32;
                    if !f.is_finite() || !(0.0..=1.0).contains(&f) {
                        return tool_err(
                            "VALIDATION_ERROR",
                            format!("`profile_strength` must be within 0.0..=1.0 (got {f})."),
                            None,
                        );
                    }
                    Some(f)
                }
                None => {
                    return tool_err(
                        "VALIDATION_ERROR",
                        "`profile_strength` must be a number within 0.0..=1.0.",
                        None,
                    );
                }
            },
        };

        let wb_pick_xy = match parse_norm_xy(args.get("wb_pick"), "wb_pick") {
            Ok(xy) => xy,
            Err(e) => return e,
        };
        let film_base_pick_xy = match parse_norm_xy(args.get("film_base_pick"), "film_base_pick") {
            Ok(xy) => xy,
            Err(e) => return e,
        };
        let skin_pick_xy = match parse_norm_xy(args.get("skin_pick"), "skin_pick") {
            Ok(xy) => xy,
            Err(e) => return e,
        };
        let color_slice_pick = match parse_color_slice_pick(args.get("color_slice_pick")) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let processing_mode_arg = match args.get("processing_mode") {
            None => None,
            Some(v) => match v.as_str() {
                Some(s) => match ProcessingMode::from_cli_str(s) {
                    Some(m) => Some(m),
                    None => {
                        return tool_err(
                            "VALIDATION_ERROR",
                            format!("Unknown `processing_mode` '{s}'."),
                            Some("Valid: photography, film_negative"),
                        );
                    }
                },
                None => {
                    return tool_err(
                        "VALIDATION_ERROR",
                        "`processing_mode` must be a string.",
                        Some("Valid: photography, film_negative"),
                    );
                }
            },
        };
        let wb_preset_arg = match args.get("wb_preset") {
            None => None,
            Some(v) => match v.as_str() {
                Some(s) => match WbPreset::from_cli_str(s) {
                    Some(p) => Some(p),
                    None => {
                        return tool_err(
                            "VALIDATION_ERROR",
                            format!("Unknown `wb_preset` '{s}'."),
                            Some(
                                "Valid: as_shot, daylight, cloudy, shade, tungsten, fluorescent, flash, custom",
                            ),
                        );
                    }
                },
                None => {
                    return tool_err(
                        "VALIDATION_ERROR",
                        "`wb_preset` must be a string.",
                        Some(
                            "Valid: as_shot, daylight, cloudy, shade, tungsten, fluorescent, flash, custom",
                        ),
                    );
                }
            },
        };
        let film_curve_arg = match args.get("film_curve") {
            None => None,
            Some(v) => match v.as_str() {
                Some(s) => match FilmCurve::from_cli_str(s) {
                    Some(c) => Some(c),
                    None => {
                        return tool_err(
                            "VALIDATION_ERROR",
                            format!("Unknown `film_curve` '{s}'."),
                            Some(
                                "Valid: film_standard, film_extra_shadow, film_high_contrast, linear_response",
                            ),
                        );
                    }
                },
                None => {
                    return tool_err(
                        "VALIDATION_ERROR",
                        "`film_curve` must be a string.",
                        Some(
                            "Valid: film_standard, film_extra_shadow, film_high_contrast, linear_response",
                        ),
                    );
                }
            },
        };
        let vignette_method_arg = match args.get("vignette_method") {
            None => None,
            Some(v) => match v.as_str() {
                Some(s) => match VignetteMethod::from_cli_str(s) {
                    Some(m) => Some(m),
                    None => {
                        return tool_err(
                            "VALIDATION_ERROR",
                            format!("Unknown `vignette_method` '{s}'."),
                            Some("Valid: elliptic_on_crop, circular_on_crop, circular"),
                        );
                    }
                },
                None => {
                    return tool_err(
                        "VALIDATION_ERROR",
                        "`vignette_method` must be a string.",
                        Some("Valid: elliptic_on_crop, circular_on_crop, circular"),
                    );
                }
            },
        };

        let (path_opt, img, mut meta, is_active) = match self.resolve_target(args, 320) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let dry_run = args
            .get("dry_run")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let agent_name = args
            .get("agent")
            .and_then(Value::as_str)
            .unwrap_or("mcp-agent");

        let before = meta.recipe.clone();
        let src_dims = Some([img.width, img.height]);

        if let Some(group) = args.get("reset").and_then(Value::as_str)
            && let Err(e) = reset_recipe_group(&mut meta.recipe, group)
        {
            return tool_err("VALIDATION_ERROR", e.to_string(), None);
        }

        if let Some(preset_id) = args.get("preset").and_then(Value::as_str) {
            let mix = args.get("mix").and_then(Value::as_f64).unwrap_or(1.0) as f32;
            match resolve_builtin_preset(preset_id) {
                Ok(mut preset) => {
                    preset.geometry = meta.recipe.geometry;
                    meta.recipe = blend_recipes(&meta.recipe, &preset, mix);
                    if (mix - 1.0).abs() <= 1e-4 {
                        meta.recipe.preset_name = preset.preset_name;
                    }
                }
                Err(e) => {
                    return tool_err(
                        "UNKNOWN_PRESET",
                        e.to_string(),
                        Some(
                            "Valid presets: neutral, warm_film, classic_chrome, editorial_bw, moody_dusk, alpine_crisp",
                        ),
                    );
                }
            }
        }

        if let Some(autos) = args.get("auto").and_then(Value::as_array) {
            for mode in autos.iter().filter_map(Value::as_str) {
                match mode.to_ascii_lowercase().as_str() {
                    "wb" => {
                        let (t, ti) = auto_white_balance(&img.histogram_proxy);
                        meta.recipe.temperature = t;
                        meta.recipe.tint = ti;
                        meta.recipe.wb_preset = WbPreset::Custom;
                    }
                    "tone" | "exposure" => {
                        let (e, c, hi, sh) = auto_exposure_and_tone(&img.histogram_proxy);
                        meta.recipe.exposure = e;
                        meta.recipe.contrast = c;
                        meta.recipe.highlights = hi;
                        meta.recipe.shadows = sh;
                    }
                    "levels" => {
                        meta.recipe.levels = auto_levels(&img.histogram_proxy);
                    }
                    "all" => {
                        let (t, ti) = auto_white_balance(&img.histogram_proxy);
                        let (e, c, hi, sh) = auto_exposure_and_tone(&img.histogram_proxy);
                        meta.recipe.temperature = t;
                        meta.recipe.tint = ti;
                        meta.recipe.wb_preset = WbPreset::Custom;
                        meta.recipe.exposure = e;
                        meta.recipe.contrast = c;
                        meta.recipe.highlights = hi;
                        meta.recipe.shadows = sh;
                        meta.recipe.levels = auto_levels(&img.histogram_proxy);
                    }
                    other => {
                        return tool_err(
                            "VALIDATION_ERROR",
                            format!("Unknown auto mode '{other}'"),
                            Some("Valid: wb, tone, levels, all"),
                        );
                    }
                }
                meta.recipe.preset_name = None;
            }
        }

        if let Some(pm) = processing_mode_arg {
            meta.recipe.processing_mode = pm;
            meta.recipe.preset_name = None;
        }

        if let Some([nx, ny]) = film_base_pick_xy {
            let raw_rgb = sample_raw_linear_rgb_at_uv(&img, nx, ny, &meta.recipe);
            meta.recipe.convert_film_negative_from_base(raw_rgb);
            meta.recipe.preset_name = None;
        }

        if let Some([nx, ny]) = wb_pick_xy {
            let patch_rgb = sample_patch_3x3(&img, nx, ny);
            let (t, ti) = pick_white_balance(patch_rgb);
            meta.recipe.temperature = t;
            meta.recipe.tint = ti;
            meta.recipe.wb_preset = WbPreset::Custom;
            meta.recipe.preset_name = None;
        }

        if let Some(wb) = wb_preset_arg {
            meta.recipe.apply_wb_preset(wb);
            meta.recipe.preset_name = None;
        }

        if let Some(profile_id) = camera_profile_arg {
            meta.recipe.camera_profile = profile_id;
            meta.recipe.preset_name = None;
        }

        if let Some(strength) = profile_strength_arg {
            meta.recipe.profile_strength = strength;
            meta.recipe.preset_name = None;
        }

        if let Some(fc) = film_curve_arg {
            meta.recipe.film_curve = fc;
            meta.recipe.preset_name = None;
        }

        if let Some(vm) = vignette_method_arg {
            meta.recipe.vignette_method = vm;
            meta.recipe.preset_name = None;
        }

        if let Some([nx, ny]) = skin_pick_xy {
            let rgb = sample_linear_rgb_at_uv(&img, nx, ny, &meta.recipe);
            meta.recipe.pick_skin_tone_from_rgb(rgb);
            meta.recipe.preset_name = None;
        }

        if let Some((slice_idx, [nx, ny])) = color_slice_pick {
            let rgb = sample_linear_rgb_at_uv(&img, nx, ny, &meta.recipe);
            meta.recipe.pick_color_slice_from_rgb(slice_idx, rgb);
            meta.recipe.preset_name = None;
        }

        if let Some(patch_val) = args.get("patch") {
            let patch: RecipePatch = match serde_json::from_value(patch_val.clone()) {
                Ok(p) => p,
                Err(e) => {
                    return tool_err(
                        "VALIDATION_ERROR",
                        format!("Invalid `patch` object: {e}"),
                        Some("Call `rune_get_schema` to inspect exact parameter names."),
                    );
                }
            };
            if let Err(e) = patch.apply_to(&mut meta.recipe, src_dims) {
                return tool_err("VALIDATION_ERROR", e.to_string(), None);
            }
        }

        if let Some(sets) = args.get("set").and_then(Value::as_object)
            && let Err(err) = apply_set_map(&mut meta.recipe, "", sets, src_dims)
        {
            return err;
        }

        meta.recipe = meta.recipe.sanitized();
        let diffs = recipe_field_diffs(&before, &meta.recipe);
        let mut live_updated = false;

        if !dry_run && !diffs.is_empty() {
            let summary = format!("MCP ({agent_name}): {} field(s)", diffs.len());
            if let Some(path) = &path_opt {
                meta.push_history(
                    EditActor::Agent {
                        name: agent_name.to_owned(),
                        job_id: None,
                    },
                    summary.clone(),
                    diffs.clone(),
                    Some(before.clone()),
                );
                if let Err(e) = meta.save_sidecar(path) {
                    return tool_err("IO_ERROR", e.to_string(), None);
                }
            }
            if is_active {
                match self
                    .host
                    .apply_live_recipe(path_opt.as_deref(), meta.recipe.clone(), summary)
                {
                    Ok(applied) => live_updated = applied,
                    Err(e) => {
                        return tool_err("LIVE_CANVAS_DISABLED", e, None);
                    }
                }
            }
        }

        let hist = compute_histogram(&img.histogram_proxy, &meta.recipe);
        let data = json!({
            "ok": true,
            "dry_run": dry_run,
            "path": path_opt,
            "live_gui_updated": live_updated,
            "wb_kelvin": temperature_to_kelvin(meta.recipe.temperature),
            "camera_profile": meta.recipe.camera_profile,
            "profile_strength": meta.recipe.profile_strength,
            "processing_mode": meta.recipe.processing_mode,
            "changed": diffs,
            "telemetry": {
                "shadow_clip_pct": (hist.shadow_clip_pct * 100.0).round() / 100.0,
                "highlight_clip_pct": (hist.highlight_clip_pct * 100.0).round() / 100.0,
            },
            "recipe_overrides": recipe_non_default_map(&meta.recipe),
        });
        tool_ok(data, None, None)
    }

    fn tool_preview_photo(&self, args: &Value) -> Value {
        let max_edge = args
            .get("max_edge")
            .and_then(Value::as_u64)
            .unwrap_or(1024)
            .clamp(128, 1600) as u32;
        let quality = args
            .get("quality")
            .and_then(Value::as_u64)
            .unwrap_or(82)
            .clamp(30, 96) as u8;
        let overwrite = args
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let out_dest = args
            .get("out")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);

        let (path_opt, img, meta, is_active) = match self.resolve_target(args, max_edge) {
            Ok(v) => v,
            Err(e) => return e,
        };

        if let Some(dest) = &out_dest
            && let Err(e) = validate_mcp_output_path(dest, path_opt.as_deref(), overwrite)
        {
            return e;
        }

        let jpeg_bytes = match export_image_to_jpeg_vec(&img, &meta.recipe, max_edge, quality) {
            Ok(bytes) => bytes,
            Err(e) => return tool_err("RENDER_FAILED", e.to_string(), None),
        };

        let mut saved_path: Option<PathBuf> = None;
        if let Some(dest) = out_dest {
            if let Some(parent) = dest.parent()
                && !parent.as_os_str().is_empty()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                return tool_err("IO_ERROR", e.to_string(), None);
            }
            if let Err(e) = rune_editor_core::session::atomic_write_bytes(&dest, &jpeg_bytes) {
                return tool_err("IO_ERROR", e.to_string(), None);
            }
            saved_path = Some(dest);
        }

        let hist = compute_histogram(&img.histogram_proxy, &meta.recipe);
        let data = json!({
            "ok": true,
            "path": path_opt,
            "saved_path": saved_path,
            "active_gui_canvas": is_active,
            "max_edge": max_edge,
            "jpeg_bytes": jpeg_bytes.len(),
            "telemetry": {
                "shadow_clip_pct": (hist.shadow_clip_pct * 100.0).round() / 100.0,
                "highlight_clip_pct": (hist.highlight_clip_pct * 100.0).round() / 100.0,
            },
            "recipe_overrides": recipe_non_default_map(&meta.recipe),
        });
        tool_ok(data, None, Some(&jpeg_bytes))
    }

    fn tool_diff_photo(&self, args: &Value) -> Value {
        let assertion_rules = match parse_diff_assertions(args.get("assertions")) {
            Ok(rules) => rules,
            Err(e) => return e,
        };

        let (path_opt, img, meta, _) = match self.resolve_target(args, 320) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let baseline = if let Some(preset_id) = args.get("against_preset").and_then(Value::as_str) {
            match resolve_builtin_preset(preset_id) {
                Ok(r) => r,
                Err(e) => return tool_err("UNKNOWN_PRESET", e.to_string(), None),
            }
        } else {
            DevelopRecipe::default()
        };

        let diff = diff_recipes(&img.histogram_proxy, &baseline, &meta.recipe);
        let mut violations = Vec::new();
        if !assertion_rules.is_empty() {
            let analysis = analyze_linear_proxy(&img.histogram_proxy, &meta.recipe, None);
            for (metric, limit) in &assertion_rules {
                let actual = match metric.as_str() {
                    "max_highlight_clip_pct" => analysis.tone.clip_pct.highlight_any,
                    "max_shadow_clip_pct" => analysis.tone.clip_pct.shadow_all,
                    "max_skin_de2000" => diff.delta_e2000.skin_tones,
                    "max_mean_de2000" => diff.delta_e2000.mean,
                    _ => unreachable!("validated by parse_diff_assertions"),
                };
                if actual > *limit {
                    violations.push(format!("{metric}: actual {actual:.2} > limit {limit:.2}"));
                }
            }
        }

        let assertions_passed = violations.is_empty();
        let data = json!({
            "ok": assertions_passed,
            "path": path_opt,
            "diff": diff,
            "assertions_passed": assertions_passed,
            "violations": violations,
        });
        if assertions_passed {
            tool_ok(data, None, None)
        } else {
            let text = serde_json::to_string(&data).unwrap_or_else(|_| "{}".to_owned());
            json!({
                "content": [{
                    "type": "text",
                    "text": text,
                }],
                "structuredContent": data,
                "isError": true,
            })
        }
    }

    fn tool_cull_or_export(&self, args: &Value) -> Value {
        if let Some(obj) = args.as_object() {
            const ALLOWED: &[&str] = &[
                "action",
                "path",
                "rating",
                "flag",
                "color_tag",
                "out",
                "overwrite",
                "format",
                "quality",
                "max_edge",
                "naming_template",
                "watermark",
                "strip_metadata",
                "color_space",
                "recipes",
            ];
            for key in obj.keys() {
                if !ALLOWED.contains(&key.as_str()) {
                    return tool_err(
                        "UNKNOWN_ARGUMENT",
                        format!("Unknown argument '{key}' on rune_cull_or_export."),
                        None,
                    );
                }
            }
        }

        let action = args.get("action").and_then(Value::as_str).unwrap_or("cull");

        match action {
            "cull" => {
                let rating = args
                    .get("rating")
                    .and_then(Value::as_u64)
                    .map(|r| (r as u8).min(5));
                let flag = match args.get("flag") {
                    None => None,
                    Some(v) => match v.as_str() {
                        Some(s) => match s.to_ascii_lowercase().as_str() {
                            "pick" => Some(CullFlag::Pick),
                            "reject" => Some(CullFlag::Reject),
                            "none" | "unflag" => Some(CullFlag::None),
                            other => {
                                return tool_err(
                                    "VALIDATION_ERROR",
                                    format!("Unknown `flag` '{other}'."),
                                    Some("Valid: pick, reject, none"),
                                );
                            }
                        },
                        None => {
                            return tool_err("VALIDATION_ERROR", "`flag` must be a string.", None);
                        }
                    },
                };
                let color_tag = match args.get("color_tag") {
                    None => None,
                    Some(v) => match v.as_str() {
                        Some(s) => match ColorTag::from_cli_str(s) {
                            Some(t) => Some(t),
                            None => {
                                return tool_err(
                                    "VALIDATION_ERROR",
                                    format!("Unknown `color_tag` '{s}'."),
                                    Some(
                                        "Valid: none, red, orange, yellow, green, blue, purple, pink",
                                    ),
                                );
                            }
                        },
                        None => {
                            return tool_err(
                                "VALIDATION_ERROR",
                                "`color_tag` must be a string.",
                                None,
                            );
                        }
                    },
                };
                let path_opt = args
                    .get("path")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from);
                if let Some(p) = &path_opt {
                    if !p.exists() {
                        return tool_err(
                            "INPUT_NOT_FOUND",
                            format!("Not found: {}", p.display()),
                            None,
                        );
                    }
                    let mut meta = PhotoMeta::load_sidecar(p)
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    if let Some(r) = rating {
                        meta.rating = r;
                    }
                    if let Some(f) = flag {
                        meta.flag = f;
                    }
                    if let Some(t) = color_tag {
                        meta.color_tag = t;
                    }
                    if let Err(e) = meta.save_sidecar_in_place(p) {
                        return tool_err("IO_ERROR", e.to_string(), None);
                    }
                }
                let live = self
                    .host
                    .apply_live_cull(path_opt.as_deref(), rating, flag, color_tag)
                    .unwrap_or(false);
                tool_ok(
                    json!({
                        "ok": true,
                        "action": "cull",
                        "path": path_opt,
                        "rating": rating,
                        "flag": flag,
                        "color_tag": color_tag,
                        "live_gui_updated": live,
                    }),
                    None,
                    None,
                )
            }
            "export" => {
                let Some(dest_str) = args
                    .get("out")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    return tool_err(
                        "VALIDATION_ERROR",
                        "Missing `out` destination path for export.",
                        None,
                    );
                };
                let dest = PathBuf::from(dest_str);
                let overwrite = args
                    .get("overwrite")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let format_str = args.get("format").and_then(Value::as_str).unwrap_or("jpeg");
                let quality = args
                    .get("quality")
                    .and_then(Value::as_u64)
                    .unwrap_or(92)
                    .clamp(1, 100) as u8;
                let max_edge = args
                    .get("max_edge")
                    .and_then(Value::as_u64)
                    .map(|v| v as u32);
                let strip_metadata = args
                    .get("strip_metadata")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let color_space = match args.get("color_space") {
                    None => ExportColorSpace::Srgb,
                    Some(v) => match v.as_str().and_then(ExportColorSpace::from_cli_str) {
                        Some(cs) => cs,
                        None => {
                            return tool_err(
                                "VALIDATION_ERROR",
                                "Invalid `color_space` (expected `srgb` or `display_p3` / `display-p3`).",
                                None,
                            );
                        }
                    },
                };
                let watermark = match parse_mcp_watermark(args.get("watermark")) {
                    Ok(w) => w,
                    Err(e) => return e,
                };
                let naming_template = args
                    .get("naming_template")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
                let format = match format_str.to_ascii_lowercase().as_str() {
                    "jpeg" | "jpg" => ExportFormat::Jpeg { quality },
                    "png8" | "png" => ExportFormat::Png8,
                    "png16" => ExportFormat::Png16,
                    "tiff16" | "tiff" | "tif" => ExportFormat::Tiff16,
                    "webp" => ExportFormat::WebP,
                    other => {
                        return tool_err(
                            "VALIDATION_ERROR",
                            format!("Unsupported export format '{other}'"),
                            Some("Valid: jpeg, png8, png16, tiff16, webp"),
                        );
                    }
                };

                let base_opts = ExportOptions {
                    format,
                    max_edge,
                    respect_crop: true,
                    output_sharpen: 0.2,
                    strip_metadata,
                    watermark,
                    color_space,
                };

                let (src_img, meta, src_path, display_stem) = if let Some(p_str) = args
                    .get("path")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    let p = PathBuf::from(p_str);
                    if !p.exists() {
                        return tool_err(
                            "INPUT_NOT_FOUND",
                            format!("Photo not found: {}", p.display()),
                            None,
                        );
                    }
                    let meta = PhotoMeta::load_sidecar(&p)
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    let stem = p
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "photo".to_owned());
                    let img = match load_file(&p) {
                        Ok(i) => Arc::new(i),
                        Err(e) => return tool_err("DECODE_FAILED", e.to_string(), None),
                    };
                    (img, meta, Some(p), stem)
                } else if let Some(snap) = self.host.active_canvas() {
                    let mut meta = snap
                        .source_path
                        .as_deref()
                        .and_then(|p| PhotoMeta::load_sidecar(p).ok().flatten())
                        .unwrap_or_default();
                    meta.recipe = snap.recipe;
                    let stem = snap
                        .source_path
                        .as_deref()
                        .and_then(|p| p.file_stem())
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| {
                            Path::new(&snap.display_name)
                                .file_stem()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "photo".to_owned())
                        });
                    (snap.image, meta, snap.source_path, stem)
                } else {
                    return tool_err(
                        "MISSING_PATH",
                        "No `path` provided and no active GUI photo.",
                        None,
                    );
                };

                let naming_ctx = NamingContext {
                    image_name: display_stem,
                    counter: 1,
                    date: String::new(),
                    camera: src_img
                        .exif
                        .as_ref()
                        .and_then(|e| e.camera_model.clone())
                        .unwrap_or_default(),
                    lens: src_img
                        .exif
                        .as_ref()
                        .and_then(|e| e.lens_model.clone())
                        .unwrap_or_default(),
                    iso: src_img
                        .exif
                        .as_ref()
                        .and_then(|e| e.iso)
                        .map(|i| format!("ISO{i}"))
                        .unwrap_or_default(),
                    variant_index: meta.active_variant_index,
                    recipe_name: format_str.to_owned(),
                };

                if let Some(recipes_val) = args.get("recipes") {
                    let recipes = match parse_mcp_export_recipes(
                        recipes_val,
                        &base_opts,
                        naming_template.as_deref(),
                    ) {
                        Ok(r) => r,
                        Err(e) => return e,
                    };
                    let planned = match plan_multi_recipe_paths(
                        &dest,
                        &recipes,
                        &naming_ctx,
                        src_path.as_deref(),
                        overwrite,
                    ) {
                        Ok(p) => p,
                        Err(e) => {
                            let code = match e {
                                rune_editor_decode::DecodeError::DestinationExists(_) => {
                                    "FILE_EXISTS"
                                }
                                _ => "VALIDATION_ERROR",
                            };
                            return tool_err(code, e.to_string(), None);
                        }
                    };
                    for (_, out_path) in &planned {
                        if let Err(e) =
                            validate_mcp_output_path(out_path, src_path.as_deref(), overwrite)
                        {
                            return e;
                        }
                    }
                    let written = match export_multi_recipe(
                        &src_img,
                        &meta.recipe,
                        &dest,
                        &recipes,
                        &naming_ctx,
                        src_path.as_deref(),
                        overwrite,
                    ) {
                        Ok(items) => items,
                        Err(e) => return tool_err("EXPORT_FAILED", e.to_string(), None),
                    };
                    return tool_ok(
                        json!({
                            "ok": true,
                            "action": "export",
                            "multi_recipe": true,
                            "source": src_path,
                            "out_dir": dest,
                            "outputs": written,
                        }),
                        None,
                        None,
                    );
                }

                let final_dest = if let Some(tpl) = &naming_template
                    && (dest.is_dir() || dest.extension().is_none())
                {
                    let stem = expand_naming_template(tpl, &naming_ctx);
                    dest.join(format!("{stem}.{}", format.extension()))
                } else {
                    dest
                };

                if let Err(e) =
                    validate_mcp_output_path(&final_dest, src_path.as_deref(), overwrite)
                {
                    return e;
                }

                if let Err(e) =
                    export_image_with_options(&src_img, &meta.recipe, &final_dest, &base_opts)
                {
                    return tool_err("EXPORT_FAILED", e.to_string(), None);
                }
                tool_ok(
                    json!({
                        "ok": true,
                        "action": "export",
                        "source": src_path,
                        "out": final_dest,
                        "format": format_str,
                        "color_space": color_space.cli_slug(),
                        "strip_metadata": strip_metadata,
                        "watermark_active": base_opts.watermark.is_active(),
                    }),
                    None,
                    None,
                )
            }
            other => tool_err(
                "VALIDATION_ERROR",
                format!("Unknown action '{other}' (expected `cull` or `export`)"),
                None,
            ),
        }
    }

    fn tool_manage_workflow(&self, args: &Value) -> Value {
        if let Some(obj) = args.as_object() {
            const ALLOWED: &[&str] = &[
                "action",
                "path",
                "dir",
                "src_path",
                "dst_path",
                "label",
                "index",
                "name",
                "description",
                "groups",
                "autoselect_adjusted",
                "stack",
                "strength",
                "camera_make",
                "camera_model",
            ];
            for key in obj.keys() {
                if !ALLOWED.contains(&key.as_str()) {
                    return tool_err(
                        "UNKNOWN_ARGUMENT",
                        format!("Unknown argument '{key}' on rune_manage_workflow."),
                        None,
                    );
                }
            }
        }

        let Some(action) = args.get("action").and_then(Value::as_str) else {
            return tool_err(
                "VALIDATION_ERROR",
                "Missing required `action` on rune_manage_workflow.",
                Some(
                    "Valid: variant_list, variant_new, variant_clone, variant_select, variant_delete, style_list, style_save, style_apply, style_delete, copy_selective_groups, session_init, profile_list",
                ),
            );
        };
        if action != "session_init" && args.get("dir").is_some() {
            return tool_err(
                "VALIDATION_ERROR",
                "`dir` is only accepted with `action: \"session_init\"`.",
                None,
            );
        }
        if action != "profile_list"
            && (args.get("camera_make").is_some() || args.get("camera_model").is_some())
        {
            return tool_err(
                "VALIDATION_ERROR",
                "`camera_make` and `camera_model` are only accepted with `action: \"profile_list\"`.",
                None,
            );
        }

        let styles_dir = self
            .styles_dir_override
            .clone()
            .unwrap_or_else(default_styles_dir);
        let profiles_dir = self
            .profiles_dir_override
            .clone()
            .unwrap_or_else(default_profiles_dir);

        let resolve_photo_path = |field: &str| -> Result<PathBuf, Value> {
            if let Some(p_str) = args
                .get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let p = PathBuf::from(p_str);
                if !p.exists() {
                    return Err(tool_err(
                        "INPUT_NOT_FOUND",
                        format!("Photo does not exist: {}", p.display()),
                        None,
                    ));
                }
                Ok(p)
            } else if let Some(snap) = self.host.active_canvas()
                && let Some(p) = snap.source_path
            {
                Ok(p)
            } else {
                Err(tool_err(
                    "MISSING_PATH",
                    format!(
                        "Provide `{field}` pointing to a photo on disk (or open a disk photo in the GUI)."
                    ),
                    None,
                ))
            }
        };

        let parse_groups = || -> Result<Vec<String>, Value> {
            match args.get("groups") {
                None => Ok(Vec::new()),
                Some(Value::Array(arr)) => {
                    let mut out = Vec::with_capacity(arr.len());
                    for item in arr {
                        let Some(s) = item.as_str() else {
                            return Err(tool_err(
                                "VALIDATION_ERROR",
                                "`groups` entries must be strings.",
                                None,
                            ));
                        };
                        out.push(s.to_owned());
                    }
                    Ok(out)
                }
                Some(_) => Err(tool_err(
                    "VALIDATION_ERROR",
                    "`groups` must be an array of tool group names.",
                    None,
                )),
            }
        };

        match action {
            "variant_list" => {
                let path = match resolve_photo_path("path") {
                    Ok(p) => p,
                    Err(e) => return e,
                };
                let mut meta = match PhotoMeta::load_sidecar(&path) {
                    Ok(m) => m.unwrap_or_default(),
                    Err(e) => return tool_err("IO_ERROR", e.to_string(), None),
                };
                meta.ensure_variants();
                tool_ok(
                    json!({
                        "ok": true,
                        "action": action,
                        "path": path,
                        "active_variant_index": meta.active_variant_index,
                        "variant_count": meta.variant_count(),
                        "variants": variants_summary_json(&meta),
                    }),
                    None,
                    None,
                )
            }
            "variant_new" | "variant_clone" => {
                let path = match resolve_photo_path("path") {
                    Ok(p) => p,
                    Err(e) => return e,
                };
                let label = args.get("label").and_then(Value::as_str).map(str::to_owned);
                let mut meta = match PhotoMeta::load_sidecar(&path) {
                    Ok(m) => m.unwrap_or_default(),
                    Err(e) => return tool_err("IO_ERROR", e.to_string(), None),
                };
                let new_idx = if action == "variant_new" {
                    meta.new_variant(label)
                } else {
                    meta.clone_variant(label)
                };
                if let Err(e) = meta.save_sidecar_in_place(&path) {
                    return tool_err("IO_ERROR", e.to_string(), None);
                }
                let live = self.sync_live_if_active(
                    &path,
                    &meta.recipe,
                    format!("MCP {action} (V{})", new_idx + 1),
                );
                tool_ok(
                    json!({
                        "ok": true,
                        "action": action,
                        "path": path,
                        "created_index": new_idx,
                        "active_variant_index": meta.active_variant_index,
                        "variant_count": meta.variant_count(),
                        "variants": variants_summary_json(&meta),
                        "live_gui_updated": live,
                    }),
                    None,
                    None,
                )
            }
            "variant_select" | "variant_delete" => {
                let path = match resolve_photo_path("path") {
                    Ok(p) => p,
                    Err(e) => return e,
                };
                let Some(index) = args
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|n| n as usize)
                else {
                    return tool_err(
                        "VALIDATION_ERROR",
                        format!("Missing integer `index` for `{action}`."),
                        None,
                    );
                };
                let mut meta = match PhotoMeta::load_sidecar(&path) {
                    Ok(m) => m.unwrap_or_default(),
                    Err(e) => return tool_err("IO_ERROR", e.to_string(), None),
                };
                let ok = if action == "variant_select" {
                    meta.select_variant(index)
                } else {
                    meta.delete_variant(index)
                };
                if !ok {
                    return tool_err(
                        "VALIDATION_ERROR",
                        format!(
                            "`{action}` failed for index {index} (photo has {} variant(s))",
                            meta.variant_count()
                        ),
                        None,
                    );
                }
                if let Err(e) = meta.save_sidecar_in_place(&path) {
                    return tool_err("IO_ERROR", e.to_string(), None);
                }
                let live = self.sync_live_if_active(
                    &path,
                    &meta.recipe,
                    format!("MCP {action} (V{})", meta.active_variant_index + 1),
                );
                tool_ok(
                    json!({
                        "ok": true,
                        "action": action,
                        "path": path,
                        "active_variant_index": meta.active_variant_index,
                        "variant_count": meta.variant_count(),
                        "variants": variants_summary_json(&meta),
                        "live_gui_updated": live,
                    }),
                    None,
                    None,
                )
            }
            "style_list" => match list_user_styles_in_dir(&styles_dir) {
                Ok(styles) => {
                    let list: Vec<Value> = styles
                        .iter()
                        .map(|s| {
                            json!({
                                "name": s.name,
                                "slug": s.slug().unwrap_or_default(),
                                "description": s.description,
                                "groups": s.groups,
                            })
                        })
                        .collect();
                    tool_ok(
                        json!({
                            "ok": true,
                            "action": action,
                            "styles_dir": styles_dir,
                            "count": list.len(),
                            "styles": list,
                        }),
                        None,
                        None,
                    )
                }
                Err(e) => tool_err("IO_ERROR", e.to_string(), None),
            },
            "style_save" => {
                let Some(name) = args.get("name").and_then(Value::as_str) else {
                    return tool_err(
                        "VALIDATION_ERROR",
                        "Missing string `name` for `style_save`.",
                        None,
                    );
                };
                let description = args
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let autoselect = args
                    .get("autoselect_adjusted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let groups = match parse_groups() {
                    Ok(g) => g,
                    Err(e) => return e,
                };
                let recipe = if let Some(p_str) = args
                    .get("path")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    let p = PathBuf::from(p_str);
                    if !p.exists() {
                        return tool_err(
                            "INPUT_NOT_FOUND",
                            format!("Photo not found: {}", p.display()),
                            None,
                        );
                    }
                    match PhotoMeta::load_sidecar(&p) {
                        Ok(m) => m.unwrap_or_default().recipe,
                        Err(e) => return tool_err("IO_ERROR", e.to_string(), None),
                    }
                } else if let Some(snap) = self.host.active_canvas() {
                    snap.recipe
                } else {
                    return tool_err(
                        "MISSING_PATH",
                        "Provide `path` or connect to an active GUI canvas for `style_save`.",
                        None,
                    );
                };
                let effective_groups: Vec<String> = if autoselect && groups.is_empty() {
                    modified_recipe_groups(&recipe)
                        .into_iter()
                        .map(str::to_owned)
                        .collect()
                } else {
                    groups
                };
                let style =
                    match UserStyle::from_recipe(name, description, &recipe, &effective_groups) {
                        Ok(s) => s,
                        Err(e) => return tool_err("VALIDATION_ERROR", e.to_string(), None),
                    };
                let saved_path = match save_user_style_in_dir(&styles_dir, &style) {
                    Ok(p) => p,
                    Err(e) => return tool_err("IO_ERROR", e.to_string(), None),
                };
                tool_ok(
                    json!({
                        "ok": true,
                        "action": action,
                        "name": style.name,
                        "slug": style.slug().unwrap_or_default(),
                        "groups": style.groups,
                        "path": saved_path,
                    }),
                    None,
                    None,
                )
            }
            "style_apply" => {
                let Some(name) = args.get("name").and_then(Value::as_str) else {
                    return tool_err(
                        "VALIDATION_ERROR",
                        "Missing string `name` for `style_apply`.",
                        None,
                    );
                };
                let stack = args.get("stack").and_then(Value::as_bool).unwrap_or(false);
                let strength = args.get("strength").and_then(Value::as_f64).unwrap_or(1.0) as f32;
                if !strength.is_finite() || !(0.0..=1.0).contains(&strength) {
                    return tool_err(
                        "VALIDATION_ERROR",
                        format!("`strength` must be within 0.0..=1.0 (got {strength})."),
                        None,
                    );
                }
                let style = match load_user_style_in_dir(&styles_dir, name) {
                    Ok(s) => s,
                    Err(e) => return tool_err("VALIDATION_ERROR", e.to_string(), None),
                };
                let (path_opt, _, mut meta, is_active) = match self.resolve_target(args, 320) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                let before = meta.recipe.clone();
                let res = if stack {
                    stack_style_onto_recipe(&mut meta.recipe, &style.patch, strength)
                } else {
                    apply_style_to_recipe(&mut meta.recipe, &style, strength)
                };
                if let Err(e) = res {
                    return tool_err("VALIDATION_ERROR", e.to_string(), None);
                }
                meta.recipe = std::mem::take(&mut meta.recipe).sanitized();
                let diffs = recipe_field_diffs(&before, &meta.recipe);
                let mut live = false;
                if !diffs.is_empty() {
                    let summary = format!(
                        "MCP style {} '{}'",
                        if stack { "stack" } else { "apply" },
                        style.name
                    );
                    if let Some(p) = &path_opt {
                        meta.push_history(
                            EditActor::Agent {
                                name: "mcp-workflow".to_owned(),
                                job_id: None,
                            },
                            summary.clone(),
                            diffs.clone(),
                            Some(before),
                        );
                        if let Err(e) = meta.save_sidecar_in_place(p) {
                            return tool_err("IO_ERROR", e.to_string(), None);
                        }
                    }
                    if is_active {
                        live = self
                            .host
                            .apply_live_recipe(path_opt.as_deref(), meta.recipe.clone(), summary)
                            .unwrap_or(false);
                    }
                }
                tool_ok(
                    json!({
                        "ok": true,
                        "action": action,
                        "name": style.name,
                        "stack": stack,
                        "strength": strength,
                        "path": path_opt,
                        "changed_count": diffs.len(),
                        "live_gui_updated": live,
                    }),
                    None,
                    None,
                )
            }
            "style_delete" => {
                let Some(name) = args.get("name").and_then(Value::as_str) else {
                    return tool_err(
                        "VALIDATION_ERROR",
                        "Missing string `name` for `style_delete`.",
                        None,
                    );
                };
                let slug = match sanitize_style_slug(name) {
                    Ok(s) => s,
                    Err(e) => return tool_err("VALIDATION_ERROR", e.to_string(), None),
                };
                match delete_user_style_in_dir(&styles_dir, name) {
                    Ok(true) => tool_ok(
                        json!({ "ok": true, "action": action, "slug": slug, "deleted": true }),
                        None,
                        None,
                    ),
                    Ok(false) => tool_err(
                        "INPUT_NOT_FOUND",
                        format!("User Style '{name}' not found."),
                        None,
                    ),
                    Err(e) => tool_err("VALIDATION_ERROR", e.to_string(), None),
                }
            }
            "copy_selective_groups" => {
                let Some(src_str) = args
                    .get("src_path")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    return tool_err(
                        "VALIDATION_ERROR",
                        "Missing `src_path` for `copy_selective_groups`.",
                        None,
                    );
                };
                let src_path = PathBuf::from(src_str);
                if !src_path.exists() {
                    return tool_err(
                        "INPUT_NOT_FOUND",
                        format!("Source photo not found: {}", src_path.display()),
                        None,
                    );
                }
                let src_recipe = match PhotoMeta::load_sidecar(&src_path) {
                    Ok(m) => m.unwrap_or_default().recipe,
                    Err(e) => return tool_err("IO_ERROR", e.to_string(), None),
                };
                let autoselect = args
                    .get("autoselect_adjusted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let mut groups = match parse_groups() {
                    Ok(g) => g,
                    Err(e) => return e,
                };
                if autoselect {
                    for g in modified_recipe_groups(&src_recipe) {
                        if !groups.iter().any(|x| x.eq_ignore_ascii_case(g)) {
                            groups.push(g.to_owned());
                        }
                    }
                }
                let dst_args = match args.get("dst_path").and_then(Value::as_str) {
                    Some(p) => json!({ "path": p }),
                    None => json!({}),
                };
                let (dst_path_opt, _, mut dst_meta, is_active) =
                    match self.resolve_target(&dst_args, 320) {
                        Ok(v) => v,
                        Err(e) => return e,
                    };
                let before = dst_meta.recipe.clone();
                if !(groups.is_empty() && autoselect)
                    && let Err(e) = copy_recipe_groups(&mut dst_meta.recipe, &src_recipe, &groups)
                {
                    return tool_err("VALIDATION_ERROR", e.to_string(), None);
                }
                dst_meta.recipe = dst_meta.recipe.sanitized();
                let diffs = recipe_field_diffs(&before, &dst_meta.recipe);
                let mut live = false;
                if !diffs.is_empty() {
                    let summary = format!("MCP copy_selective_groups ({} group(s))", groups.len());
                    if let Some(p) = &dst_path_opt {
                        dst_meta.push_history(
                            EditActor::Agent {
                                name: "mcp-workflow".to_owned(),
                                job_id: None,
                            },
                            summary.clone(),
                            diffs.clone(),
                            Some(before),
                        );
                        if let Err(e) = dst_meta.save_sidecar_in_place(p) {
                            return tool_err("IO_ERROR", e.to_string(), None);
                        }
                    }
                    if is_active {
                        live = self
                            .host
                            .apply_live_recipe(
                                dst_path_opt.as_deref(),
                                dst_meta.recipe.clone(),
                                summary,
                            )
                            .unwrap_or(false);
                    }
                }
                tool_ok(
                    json!({
                        "ok": true,
                        "action": action,
                        "src_path": src_path,
                        "dst_path": dst_path_opt,
                        "groups": groups,
                        "changed_count": diffs.len(),
                        "live_gui_updated": live,
                    }),
                    None,
                    None,
                )
            }
            "session_init" => {
                let Some(dir_str) = args
                    .get("dir")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    return tool_err(
                        "VALIDATION_ERROR",
                        "Missing non-empty string `dir` for `session_init`.",
                        None,
                    );
                };
                if dir_str.contains('\0') {
                    return tool_err(
                        "VALIDATION_ERROR",
                        "`dir` must not contain NUL bytes.",
                        None,
                    );
                }
                let dir_path = PathBuf::from(dir_str);
                if dir_path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    return tool_err(
                        "VALIDATION_ERROR",
                        "`dir` must not contain parent-directory traversal (`..`).",
                        None,
                    );
                }
                if let Ok(meta) = std::fs::symlink_metadata(&dir_path)
                    && (meta.file_type().is_symlink() || !meta.is_dir())
                {
                    return tool_err(
                        "VALIDATION_ERROR",
                        format!(
                            "`dir` must not be a symlink or existing file: {}",
                            dir_path.display()
                        ),
                        None,
                    );
                }
                for sub in ["Capture", "Selects", "Output", "Trash"] {
                    let child = dir_path.join(sub);
                    if let Ok(meta) = std::fs::symlink_metadata(&child)
                        && (meta.file_type().is_symlink() || !meta.is_dir())
                    {
                        return tool_err(
                            "VALIDATION_ERROR",
                            format!(
                                "Session subfolder cannot be a symlink or existing file: {}",
                                child.display()
                            ),
                            None,
                        );
                    }
                }
                match create_capture_one_session(&dir_path) {
                    Ok(layout) => tool_ok(
                        json!({
                            "ok": true,
                            "action": action,
                            "root": layout.root,
                            "capture": layout.capture,
                            "selects": layout.selects,
                            "output": layout.output,
                            "trash": layout.trash,
                        }),
                        None,
                        None,
                    ),
                    Err(e) => tool_err("IO_ERROR", e.to_string(), None),
                }
            }
            "profile_list" => {
                let camera_make = match args.get("camera_make") {
                    None | Some(Value::Null) => None,
                    Some(v) => match v.as_str() {
                        Some(s) => {
                            let t = s.trim();
                            (!t.is_empty()).then_some(t)
                        }
                        None => {
                            return tool_err(
                                "VALIDATION_ERROR",
                                "`camera_make` must be a string.",
                                None,
                            );
                        }
                    },
                };
                let camera_model = match args.get("camera_model") {
                    None | Some(Value::Null) => None,
                    Some(v) => match v.as_str() {
                        Some(s) => {
                            let t = s.trim();
                            (!t.is_empty()).then_some(t)
                        }
                        None => {
                            return tool_err(
                                "VALIDATION_ERROR",
                                "`camera_model` must be a string.",
                                None,
                            );
                        }
                    },
                };
                let profiles = list_all_camera_profiles(
                    camera_make,
                    camera_model,
                    Some(profiles_dir.as_path()),
                );
                let first_builtin_idx = profiles
                    .iter()
                    .position(|p| p.category != ProfileCategory::User)
                    .unwrap_or(profiles.len());
                let has_filter = camera_make.is_some() || camera_model.is_some();
                let list: Vec<Value> = profiles
                    .iter()
                    .enumerate()
                    .map(|(idx, p)| {
                        let exif_matched = if !has_filter {
                            false
                        } else if p.category == ProfileCategory::User {
                            idx < first_builtin_idx
                        } else if let Some(cm) = p.camera_match.as_deref()
                            && cm != "*"
                        {
                            let needle = cm.trim().to_ascii_lowercase();
                            let make_hit = camera_make
                                .map(str::to_ascii_lowercase)
                                .is_some_and(|m| m.contains(&needle));
                            let model_hit =
                                camera_model.map(str::to_ascii_lowercase).is_some_and(|m| {
                                    m.contains(&needle)
                                        || (needle == "apple" && m.contains("iphone"))
                                });
                            make_hit || model_hit
                        } else {
                            false
                        };
                        json!({
                            "id": p.id,
                            "name": p.name,
                            "category": p.category,
                            "category_label": p.category.label(),
                            "description": p.description,
                            "camera_match": p.camera_match,
                            "exif_matched": exif_matched,
                        })
                    })
                    .collect();
                tool_ok(
                    json!({
                        "ok": true,
                        "action": action,
                        "camera_make": camera_make,
                        "camera_model": camera_model,
                        "count": list.len(),
                        "profiles": list,
                    }),
                    None,
                    None,
                )
            }
            other => tool_err(
                "VALIDATION_ERROR",
                format!("Unknown workflow action '{other}'."),
                Some(
                    "Valid: variant_list, variant_new, variant_clone, variant_select, variant_delete, style_list, style_save, style_apply, style_delete, copy_selective_groups, session_init, profile_list",
                ),
            ),
        }
    }

    fn tool_edit_layers(&self, args: &Value) -> Value {
        if let Some(obj) = args.as_object() {
            const ALLOWED: &[&str] = &[
                "action",
                "path",
                "index",
                "id",
                "name",
                "kind",
                "opacity",
                "enabled",
                "inverted",
                "op",
                "adjustments",
                "start",
                "end",
                "center",
                "radius",
                "angle_deg",
                "feather",
                "low",
                "high",
                "low_falloff",
                "high_falloff",
                "strokes",
                "target",
                "source",
                "mode",
                "dry_run",
                "agent",
            ];
            for key in obj.keys() {
                if !ALLOWED.contains(&key.as_str()) {
                    return tool_err(
                        "UNKNOWN_ARGUMENT",
                        format!("Unknown argument '{key}' on rune_edit_layers."),
                        None,
                    );
                }
            }
        }

        let Some(action) = args.get("action").and_then(Value::as_str) else {
            return tool_err(
                "VALIDATION_ERROR",
                "Missing required `action` on rune_edit_layers.",
                Some(
                    "Valid: layer_list, layer_add, layer_update, layer_remove, mask_add, mask_clear, ai_subject, ai_background, heal_list, heal_add, heal_remove, heal_clear",
                ),
            );
        };

        let dry_run = args
            .get("dry_run")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let agent_name = args
            .get("agent")
            .and_then(Value::as_str)
            .unwrap_or("mcp-layers");

        let (path_opt, img, mut meta, is_active) = match self.resolve_target(args, 512) {
            Ok(v) => v,
            Err(e) => return e,
        };

        if action == "layer_list" {
            return tool_ok(
                json!({
                    "ok": true,
                    "action": action,
                    "path": path_opt,
                    "layer_count": meta.recipe.layers.len(),
                    "layers": layers_summary_json(&meta.recipe.layers),
                }),
                None,
                None,
            );
        }
        if action == "heal_list" {
            return tool_ok(
                json!({
                    "ok": true,
                    "action": action,
                    "path": path_opt,
                    "heal_spot_count": meta.recipe.heal_spots.len(),
                    "heal_spots": meta.recipe.heal_spots,
                }),
                None,
                None,
            );
        }

        let before = meta.recipe.clone();
        let mut extra = serde_json::Map::new();

        let commit = |meta: &mut PhotoMeta,
                      before: DevelopRecipe,
                      summary: String,
                      extra: serde_json::Map<String, Value>|
         -> Value {
            meta.recipe = std::mem::take(&mut meta.recipe).sanitized();
            let diffs = recipe_field_diffs(&before, &meta.recipe);
            let mut live_updated = false;
            if !dry_run && !diffs.is_empty() {
                if let Some(p) = &path_opt {
                    meta.push_history(
                        EditActor::Agent {
                            name: agent_name.to_owned(),
                            job_id: None,
                        },
                        summary.clone(),
                        diffs.clone(),
                        Some(before),
                    );
                    if let Err(e) = meta.save_sidecar_in_place(p) {
                        return tool_err("IO_ERROR", e.to_string(), None);
                    }
                }
                if is_active {
                    match self.host.apply_live_recipe(
                        path_opt.as_deref(),
                        meta.recipe.clone(),
                        summary,
                    ) {
                        Ok(applied) => live_updated = applied,
                        Err(e) => return tool_err("LIVE_CANVAS_DISABLED", e, None),
                    }
                }
            }
            let mut payload = json!({
                "ok": true,
                "action": action,
                "dry_run": dry_run,
                "path": path_opt,
                "live_gui_updated": live_updated,
                "layer_count": meta.recipe.layers.len(),
                "layers": layers_summary_json(&meta.recipe.layers),
                "heal_spot_count": meta.recipe.heal_spots.len(),
                "changed_count": diffs.len(),
            });
            if let Some(obj) = payload.as_object_mut() {
                for (k, v) in extra {
                    obj.insert(k, v);
                }
            }
            tool_ok(payload, None, None)
        };

        match action {
            "layer_add" => {
                if meta.recipe.layers.len() >= MAX_ADJUSTMENT_LAYERS {
                    return tool_err(
                        "VALIDATION_ERROR",
                        format!("Maximum {MAX_ADJUSTMENT_LAYERS} adjustment layers reached."),
                        None,
                    );
                }
                let name = args
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Adjustment Layer")
                    .to_owned();
                let opacity = match parse_opt_f32_range(args.get("opacity"), "opacity", 0.0, 1.0) {
                    Ok(v) => v.unwrap_or(1.0),
                    Err(e) => return e,
                };
                let inverted = args
                    .get("inverted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let kind = args.get("kind").and_then(Value::as_str).unwrap_or("empty");
                let id = next_mcp_layer_id(&meta.recipe.layers);
                let mut layer = if kind.eq_ignore_ascii_case("empty") {
                    AdjustmentLayer::new(&id, &name)
                } else {
                    let shape = match parse_mcp_mask_shape(kind, args, &img) {
                        Ok(s) => s,
                        Err(e) => return e,
                    };
                    AdjustmentLayer::new_with_mask(&id, &name, shape)
                };
                layer.opacity = opacity;
                layer.inverted = inverted;
                if let Some(en_val) = args.get("enabled") {
                    let Some(en) = en_val.as_bool() else {
                        return tool_err("VALIDATION_ERROR", "`enabled` must be a boolean.", None);
                    };
                    layer.enabled = en;
                }
                if let Err(e) =
                    apply_mcp_local_adjustments(&mut layer.adjustments, args.get("adjustments"))
                {
                    return e;
                }
                meta.recipe.layers.push(layer.sanitized());
                meta.recipe.preset_name = None;
                let idx = meta.recipe.layers.len() - 1;
                extra.insert("index".to_owned(), json!(idx));
                extra.insert("id".to_owned(), json!(id));
                commit(
                    &mut meta,
                    before,
                    format!("MCP layer_add '{name}' ({kind})"),
                    extra,
                )
            }
            "layer_update" => {
                let idx = match resolve_mcp_layer_index(&meta.recipe.layers, args) {
                    Ok(i) => i,
                    Err(e) => return e,
                };
                if let Some(op) =
                    match parse_opt_f32_range(args.get("opacity"), "opacity", 0.0, 1.0) {
                        Ok(v) => v,
                        Err(e) => return e,
                    }
                {
                    meta.recipe.layers[idx].opacity = op;
                }
                if let Some(name) = args
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    meta.recipe.layers[idx].name = name.to_owned();
                }
                if let Some(en) = args.get("enabled").and_then(Value::as_bool) {
                    meta.recipe.layers[idx].enabled = en;
                }
                if let Some(inv) = args.get("inverted").and_then(Value::as_bool) {
                    meta.recipe.layers[idx].inverted = inv;
                }
                if let Err(e) = apply_mcp_local_adjustments(
                    &mut meta.recipe.layers[idx].adjustments,
                    args.get("adjustments"),
                ) {
                    return e;
                }
                meta.recipe.layers[idx] = meta.recipe.layers[idx].clone().sanitized();
                meta.recipe.preset_name = None;
                extra.insert("index".to_owned(), json!(idx));
                commit(&mut meta, before, format!("MCP layer_update #{idx}"), extra)
            }
            "layer_remove" => {
                let idx = match resolve_mcp_layer_index(&meta.recipe.layers, args) {
                    Ok(i) => i,
                    Err(e) => return e,
                };
                let removed = meta.recipe.layers.remove(idx);
                meta.recipe.preset_name = None;
                extra.insert("removed_id".to_owned(), json!(removed.id));
                commit(
                    &mut meta,
                    before,
                    format!("MCP layer_remove '{}'", removed.name),
                    extra,
                )
            }
            "mask_add" => {
                let idx = match resolve_mcp_layer_index(&meta.recipe.layers, args) {
                    Ok(i) => i,
                    Err(e) => return e,
                };
                if meta.recipe.layers[idx].masks.len() >= MAX_MASKS_PER_LAYER {
                    return tool_err(
                        "VALIDATION_ERROR",
                        format!(
                            "Layer #{idx} already has the maximum {MAX_MASKS_PER_LAYER} masks."
                        ),
                        None,
                    );
                }
                let Some(kind) = args.get("kind").and_then(Value::as_str) else {
                    return tool_err(
                        "VALIDATION_ERROR",
                        "Missing `kind` for `mask_add` (linear, radial, luma, filled, brush, ai_subject, ai_background).",
                        None,
                    );
                };
                let op_str = args.get("op").and_then(Value::as_str).unwrap_or("add");
                let Some(combine_op) = MaskCombineOp::from_cli_str(op_str) else {
                    return tool_err(
                        "VALIDATION_ERROR",
                        format!("Unknown `op` '{op_str}' (expected add, subtract, intersect)."),
                        None,
                    );
                };
                let shape = match parse_mcp_mask_shape(kind, args, &img) {
                    Ok(s) => s,
                    Err(e) => return e,
                };
                let mut item = LayerMaskItem::new(shape);
                item.op = combine_op;
                item.inverted = args
                    .get("inverted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                meta.recipe.layers[idx].masks.push(item.sanitized());
                meta.recipe.preset_name = None;
                extra.insert("index".to_owned(), json!(idx));
                extra.insert(
                    "mask_count".to_owned(),
                    json!(meta.recipe.layers[idx].masks.len()),
                );
                commit(
                    &mut meta,
                    before,
                    format!("MCP mask_add #{idx} ({kind}, {op_str})"),
                    extra,
                )
            }
            "mask_clear" => {
                let idx = match resolve_mcp_layer_index(&meta.recipe.layers, args) {
                    Ok(i) => i,
                    Err(e) => return e,
                };
                meta.recipe.layers[idx].masks.clear();
                meta.recipe.preset_name = None;
                extra.insert("index".to_owned(), json!(idx));
                commit(&mut meta, before, format!("MCP mask_clear #{idx}"), extra)
            }
            "ai_subject" | "ai_background" => {
                let is_bg = action == "ai_background";
                let shape = generate_subject_mask(&img, is_bg);
                let opacity = match parse_opt_f32_range(args.get("opacity"), "opacity", 0.0, 1.0) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                let target_idx = if args.get("index").is_some() || args.get("id").is_some() {
                    let idx = match resolve_mcp_layer_index(&meta.recipe.layers, args) {
                        Ok(i) => i,
                        Err(e) => return e,
                    };
                    let op_str = args.get("op").and_then(Value::as_str).unwrap_or("add");
                    let Some(combine_op) = MaskCombineOp::from_cli_str(op_str) else {
                        return tool_err(
                            "VALIDATION_ERROR",
                            format!("Unknown `op` '{op_str}' (expected add, subtract, intersect)."),
                            None,
                        );
                    };
                    if meta.recipe.layers[idx].masks.len() >= MAX_MASKS_PER_LAYER {
                        return tool_err(
                            "VALIDATION_ERROR",
                            format!(
                                "Layer #{idx} already has the maximum {MAX_MASKS_PER_LAYER} masks."
                            ),
                            None,
                        );
                    }
                    let mut item = LayerMaskItem::new(shape);
                    item.op = combine_op;
                    item.inverted = args
                        .get("inverted")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    meta.recipe.layers[idx].masks.push(item.sanitized());
                    if let Some(op) = opacity {
                        meta.recipe.layers[idx].opacity = op;
                    }
                    if let Err(e) = apply_mcp_local_adjustments(
                        &mut meta.recipe.layers[idx].adjustments,
                        args.get("adjustments"),
                    ) {
                        return e;
                    }
                    idx
                } else {
                    if meta.recipe.layers.len() >= MAX_ADJUSTMENT_LAYERS {
                        return tool_err(
                            "VALIDATION_ERROR",
                            format!("Maximum {MAX_ADJUSTMENT_LAYERS} adjustment layers reached."),
                            None,
                        );
                    }
                    let default_name = if is_bg { "AI Background" } else { "AI Subject" };
                    let name = args
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .unwrap_or(default_name);
                    let id = next_mcp_layer_id(&meta.recipe.layers);
                    let mut layer = AdjustmentLayer::new_with_mask(&id, name, shape);
                    layer.opacity = opacity.unwrap_or(1.0);
                    layer.inverted = args
                        .get("inverted")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if let Err(e) =
                        apply_mcp_local_adjustments(&mut layer.adjustments, args.get("adjustments"))
                    {
                        return e;
                    }
                    meta.recipe.layers.push(layer.sanitized());
                    meta.recipe.layers.len() - 1
                };
                meta.recipe.preset_name = None;
                extra.insert("index".to_owned(), json!(target_idx));
                commit(
                    &mut meta,
                    before,
                    format!("MCP {action} (layer #{target_idx})"),
                    extra,
                )
            }
            "heal_add" => {
                if meta.recipe.heal_spots.len() >= MAX_HEAL_SPOTS {
                    return tool_err(
                        "VALIDATION_ERROR",
                        format!("Maximum {MAX_HEAL_SPOTS} heal/clone spots reached."),
                        None,
                    );
                }
                let Some(target) = (match parse_norm_xy(args.get("target"), "target") {
                    Ok(xy) => xy,
                    Err(e) => return e,
                }) else {
                    return tool_err(
                        "VALIDATION_ERROR",
                        "Missing `target` `[x, y]` for `heal_add`.",
                        None,
                    );
                };
                let radius = match parse_opt_f32_range(args.get("radius"), "radius", 0.002, 0.25) {
                    Ok(v) => v.unwrap_or(0.03),
                    Err(e) => return e,
                };
                let feather = match parse_opt_f32_range(args.get("feather"), "feather", 0.0, 1.0) {
                    Ok(v) => v.unwrap_or(0.4),
                    Err(e) => return e,
                };
                let opacity = match parse_opt_f32_range(args.get("opacity"), "opacity", 0.0, 1.0) {
                    Ok(v) => v.unwrap_or(1.0),
                    Err(e) => return e,
                };
                let mode_str = args.get("mode").and_then(Value::as_str).unwrap_or("heal");
                let Some(mode) = HealMode::from_cli_str(mode_str) else {
                    return tool_err(
                        "VALIDATION_ERROR",
                        format!("Unknown `mode` '{mode_str}' (expected heal or clone)."),
                        None,
                    );
                };
                let source = match parse_norm_xy(args.get("source"), "source") {
                    Ok(Some(xy)) => xy,
                    Ok(None) => HealSpot::auto_source_for_target(target, radius),
                    Err(e) => return e,
                };
                let id = next_mcp_heal_id(&meta.recipe.heal_spots);
                let spot = HealSpot {
                    id: id.clone(),
                    target,
                    source,
                    radius,
                    feather,
                    opacity,
                    mode,
                }
                .sanitized();
                meta.recipe.heal_spots.push(spot.clone());
                meta.recipe.preset_name = None;
                let idx = meta.recipe.heal_spots.len() - 1;
                extra.insert("index".to_owned(), json!(idx));
                extra.insert("spot".to_owned(), json!(spot));
                commit(
                    &mut meta,
                    before,
                    format!("MCP heal_add ({mode_str})"),
                    extra,
                )
            }
            "heal_remove" => {
                let idx = match resolve_mcp_heal_index(&meta.recipe.heal_spots, args) {
                    Ok(i) => i,
                    Err(e) => return e,
                };
                let removed = meta.recipe.heal_spots.remove(idx);
                meta.recipe.preset_name = None;
                extra.insert("removed_id".to_owned(), json!(removed.id));
                commit(
                    &mut meta,
                    before,
                    format!("MCP heal_remove '{}'", removed.id),
                    extra,
                )
            }
            "heal_clear" => {
                let cleared = meta.recipe.heal_spots.len();
                meta.recipe.heal_spots.clear();
                meta.recipe.preset_name = None;
                extra.insert("cleared".to_owned(), json!(cleared));
                commit(
                    &mut meta,
                    before,
                    format!("MCP heal_clear ({cleared} spots)"),
                    extra,
                )
            }
            other => tool_err(
                "VALIDATION_ERROR",
                format!("Unknown `rune_edit_layers` action '{other}'."),
                Some(
                    "Valid: layer_list, layer_add, layer_update, layer_remove, mask_add, mask_clear, ai_subject, ai_background, heal_list, heal_add, heal_remove, heal_clear",
                ),
            ),
        }
    }

    fn sync_live_if_active(&self, path: &Path, recipe: &DevelopRecipe, summary: String) -> bool {
        let is_active = self
            .host
            .active_canvas()
            .and_then(|s| s.source_path)
            .is_some_and(|p| p == path);
        if is_active {
            self.host
                .apply_live_recipe(Some(path), recipe.clone(), summary)
                .unwrap_or(false)
        } else {
            false
        }
    }
}

fn variants_summary_json(meta: &PhotoMeta) -> Vec<Value> {
    if meta.variants.is_empty() {
        vec![json!({
            "index": 0,
            "id": "v1",
            "label": "Original",
            "rating": meta.rating,
            "flag": meta.flag,
            "color_tag": meta.color_tag,
            "active": true,
        })]
    } else {
        meta.variants
            .iter()
            .enumerate()
            .map(|(i, v)| {
                json!({
                    "index": i,
                    "id": v.id,
                    "label": v.label,
                    "rating": v.rating,
                    "flag": v.flag,
                    "color_tag": v.color_tag,
                    "active": i == meta.active_variant_index,
                })
            })
            .collect()
    }
}

fn layers_summary_json(layers: &[AdjustmentLayer]) -> Vec<Value> {
    layers
        .iter()
        .enumerate()
        .map(|(i, l)| {
            json!({
                "index": i,
                "id": l.id,
                "name": l.name,
                "enabled": l.enabled,
                "opacity": l.opacity,
                "inverted": l.inverted,
                "mask_count": l.masks.len(),
                "mask_badge": l.primary_mask_badge(),
                "adjustments_neutral": l.adjustments.is_neutral(),
            })
        })
        .collect()
}

fn next_mcp_layer_id(layers: &[AdjustmentLayer]) -> String {
    let mut n = layers.len() + 1;
    loop {
        let candidate = format!("layer-{n}");
        if !layers.iter().any(|l| l.id == candidate) {
            return candidate;
        }
        n += 1;
    }
}

fn next_mcp_heal_id(spots: &[HealSpot]) -> String {
    let mut n = spots.len() + 1;
    loop {
        let candidate = format!("heal-{n}");
        if !spots.iter().any(|s| s.id == candidate) {
            return candidate;
        }
        n += 1;
    }
}

fn resolve_mcp_layer_index(layers: &[AdjustmentLayer], args: &Value) -> Result<usize, Value> {
    if let Some(idx) = args.get("index").and_then(Value::as_u64) {
        let i = idx as usize;
        if i < layers.len() {
            return Ok(i);
        }
        return Err(tool_err(
            "VALIDATION_ERROR",
            format!(
                "Layer index {i} out of range (photo has {} layer(s)).",
                layers.len()
            ),
            None,
        ));
    }
    if let Some(id) = args.get("id").and_then(Value::as_str) {
        if let Some(pos) = layers.iter().position(|l| l.id == id) {
            return Ok(pos);
        }
        return Err(tool_err(
            "VALIDATION_ERROR",
            format!("Layer id '{id}' not found."),
            None,
        ));
    }
    Err(tool_err(
        "VALIDATION_ERROR",
        "Provide integer `index` or string `id` to select a layer.",
        None,
    ))
}

fn resolve_mcp_heal_index(spots: &[HealSpot], args: &Value) -> Result<usize, Value> {
    if let Some(idx) = args.get("index").and_then(Value::as_u64) {
        let i = idx as usize;
        if i < spots.len() {
            return Ok(i);
        }
        return Err(tool_err(
            "VALIDATION_ERROR",
            format!(
                "Heal spot index {i} out of range (photo has {} spot(s)).",
                spots.len()
            ),
            None,
        ));
    }
    if let Some(id) = args.get("id").and_then(Value::as_str) {
        if let Some(pos) = spots.iter().position(|s| s.id == id) {
            return Ok(pos);
        }
        return Err(tool_err(
            "VALIDATION_ERROR",
            format!("Heal spot id '{id}' not found."),
            None,
        ));
    }
    Err(tool_err(
        "VALIDATION_ERROR",
        "Provide integer `index` or string `id` to select a heal spot.",
        None,
    ))
}

fn parse_opt_f32_range(
    val: Option<&Value>,
    field: &str,
    min: f32,
    max: f32,
) -> Result<Option<f32>, Value> {
    let Some(v) = val else {
        return Ok(None);
    };
    let Some(n) = v.as_f64() else {
        return Err(tool_err(
            "VALIDATION_ERROR",
            format!("`{field}` must be a number in {min}..={max}."),
            None,
        ));
    };
    let f = n as f32;
    if !f.is_finite() || f < min || f > max {
        return Err(tool_err(
            "VALIDATION_ERROR",
            format!("`{field}` must be within {min}..={max} (got {f})."),
            None,
        ));
    }
    Ok(Some(f))
}

fn apply_mcp_local_adjustments(
    adj: &mut LocalAdjustments,
    val: Option<&Value>,
) -> Result<(), Value> {
    let Some(v) = val else {
        return Ok(());
    };
    let Some(obj) = v.as_object() else {
        return Err(tool_err(
            "VALIDATION_ERROR",
            "`adjustments` must be an object mapping local slider names to numbers.",
            Some(
                "Valid keys: exposure, contrast, brightness, highlights, shadows, temperature, tint, saturation, clarity, dehaze, sharpen",
            ),
        ));
    };
    for (k, raw) in obj {
        let Some(n) = raw.as_f64() else {
            return Err(tool_err(
                "VALIDATION_ERROR",
                format!("Local adjustment '{k}' must be a finite number."),
                None,
            ));
        };
        let f = n as f32;
        if !f.is_finite() {
            return Err(tool_err(
                "VALIDATION_ERROR",
                format!("Local adjustment '{k}' must be finite."),
                None,
            ));
        }
        adj.set_by_name(k, f)
            .map_err(|e| tool_err("VALIDATION_ERROR", e.to_string(), None))?;
    }
    Ok(())
}

fn parse_mcp_mask_shape(kind: &str, args: &Value, img: &LinearImage) -> Result<MaskShape, Value> {
    match kind.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "filled" => Ok(MaskShape::Filled),
        "linear" | "linear_gradient" => {
            let start = parse_norm_xy(args.get("start"), "start")?.unwrap_or([0.5, 0.0]);
            let end = parse_norm_xy(args.get("end"), "end")?.unwrap_or([0.5, 0.55]);
            let feather =
                parse_opt_f32_range(args.get("feather"), "feather", 0.0, 1.0)?.unwrap_or(1.0);
            Ok(MaskShape::LinearGradient {
                start,
                end,
                feather,
            })
        }
        "radial" | "radial_gradient" => {
            let center = parse_norm_xy(args.get("center"), "center")?.unwrap_or([0.5, 0.5]);
            let radius = match args.get("radius") {
                None => [0.35, 0.35],
                Some(Value::Number(n)) => {
                    let r = n.as_f64().unwrap_or(0.35) as f32;
                    if !r.is_finite() || !(0.01..=1.5).contains(&r) {
                        return Err(tool_err(
                            "VALIDATION_ERROR",
                            format!("`radius` must be within 0.01..=1.5 (got {r})."),
                            None,
                        ));
                    }
                    [r, r]
                }
                Some(Value::Array(arr)) if arr.len() == 2 => {
                    let rx = arr[0].as_f64().unwrap_or(f64::NAN) as f32;
                    let ry = arr[1].as_f64().unwrap_or(f64::NAN) as f32;
                    if !rx.is_finite()
                        || !ry.is_finite()
                        || !(0.01..=1.5).contains(&rx)
                        || !(0.01..=1.5).contains(&ry)
                    {
                        return Err(tool_err(
                            "VALIDATION_ERROR",
                            "`radius` array `[rx, ry]` must be within 0.01..=1.5.",
                            None,
                        ));
                    }
                    [rx, ry]
                }
                Some(_) => {
                    return Err(tool_err(
                        "VALIDATION_ERROR",
                        "`radius` must be a number or `[rx, ry]` pair in 0.01..=1.5.",
                        None,
                    ));
                }
            };
            let angle_deg = parse_opt_f32_range(args.get("angle_deg"), "angle_deg", -180.0, 180.0)?
                .unwrap_or(0.0);
            let feather =
                parse_opt_f32_range(args.get("feather"), "feather", 0.0, 1.0)?.unwrap_or(0.4);
            Ok(MaskShape::RadialGradient {
                center,
                radius,
                angle_deg,
                feather,
            })
        }
        "luma" | "luma_range" => {
            let low = parse_opt_f32_range(args.get("low"), "low", 0.0, 1.0)?.unwrap_or(0.5);
            let high = parse_opt_f32_range(args.get("high"), "high", 0.0, 1.0)?.unwrap_or(1.0);
            if low > high {
                return Err(tool_err(
                    "VALIDATION_ERROR",
                    format!("`low` ({low}) must be <= `high` ({high}) for luma mask."),
                    None,
                ));
            }
            let low_falloff =
                parse_opt_f32_range(args.get("low_falloff"), "low_falloff", 0.0, 1.0)?
                    .unwrap_or(0.1);
            let high_falloff =
                parse_opt_f32_range(args.get("high_falloff"), "high_falloff", 0.0, 1.0)?
                    .unwrap_or(0.1);
            Ok(MaskShape::LumaRange {
                low,
                high,
                low_falloff,
                high_falloff,
            })
        }
        "brush" | "brush_strokes" => {
            let strokes: Vec<BrushDab> = match args.get("strokes") {
                None => Vec::new(),
                Some(v) => {
                    if let Some(arr) = v.as_array()
                        && arr.len() > MAX_BRUSH_DABS_PER_MASK
                    {
                        return Err(tool_err(
                            "VALIDATION_ERROR",
                            format!(
                                "`strokes` exceeds maximum {MAX_BRUSH_DABS_PER_MASK} dabs per mask (got {}).",
                                arr.len()
                            ),
                            None,
                        ));
                    }
                    serde_json::from_value(v.clone()).map_err(|e| {
                        tool_err(
                            "VALIDATION_ERROR",
                            format!("Invalid `strokes` array for brush mask: {e}"),
                            None,
                        )
                    })?
                }
            };
            if strokes.len() > MAX_BRUSH_DABS_PER_MASK {
                return Err(tool_err(
                    "VALIDATION_ERROR",
                    format!(
                        "`strokes` exceeds maximum {MAX_BRUSH_DABS_PER_MASK} dabs per mask (got {}).",
                        strokes.len()
                    ),
                    None,
                ));
            }
            Ok(MaskShape::BrushStrokes {
                strokes: strokes.into_iter().map(BrushDab::sanitized).collect(),
            })
        }
        "ai_subject" | "subject" => Ok(generate_subject_mask(img, false)),
        "ai_background" | "background" | "ai_bg" => Ok(generate_subject_mask(img, true)),
        other => Err(tool_err(
            "VALIDATION_ERROR",
            format!(
                "Unknown mask kind '{other}' (expected filled, linear, radial, luma, brush, ai_subject, ai_background)."
            ),
            None,
        )),
    }
}

fn parse_mcp_watermark(val: Option<&Value>) -> Result<TextWatermark, Value> {
    let Some(v) = val else {
        return Ok(TextWatermark::default());
    };
    let Some(obj) = v.as_object() else {
        return Err(tool_err(
            "VALIDATION_ERROR",
            "`watermark` must be an object `{ text, opacity?, scale?, position? }`.",
            None,
        ));
    };
    for key in obj.keys() {
        if !matches!(
            key.as_str(),
            "enabled" | "text" | "opacity" | "scale" | "position"
        ) {
            return Err(tool_err(
                "VALIDATION_ERROR",
                format!("Unknown key '{key}' in `watermark`."),
                Some("Allowed keys: enabled, text, opacity, scale, position"),
            ));
        }
    }
    let text = obj
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    let enabled = obj
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(!text.is_empty());
    let opacity = match obj.get("opacity") {
        None => 0.45_f32,
        Some(n) => {
            let Some(f) = n.as_f64() else {
                return Err(tool_err(
                    "VALIDATION_ERROR",
                    "`watermark.opacity` must be a number in 0.0..=1.0.",
                    None,
                ));
            };
            let f = f as f32;
            if !f.is_finite() || !(0.0..=1.0).contains(&f) {
                return Err(tool_err(
                    "VALIDATION_ERROR",
                    format!("`watermark.opacity` must be within 0.0..=1.0 (got {f})."),
                    None,
                ));
            }
            f
        }
    };
    let scale = match obj.get("scale") {
        None => 1.0_f32,
        Some(n) => {
            let Some(f) = n.as_f64() else {
                return Err(tool_err(
                    "VALIDATION_ERROR",
                    "`watermark.scale` must be a number in 0.5..=3.0.",
                    None,
                ));
            };
            let f = f as f32;
            if !f.is_finite() || !(0.5..=3.0).contains(&f) {
                return Err(tool_err(
                    "VALIDATION_ERROR",
                    format!("`watermark.scale` must be within 0.5..=3.0 (got {f})."),
                    None,
                ));
            }
            f
        }
    };
    let position = match obj.get("position") {
        None => WatermarkPosition::BottomRight,
        Some(p) => match p.as_str().and_then(WatermarkPosition::from_cli_str) {
            Some(pos) => pos,
            None => {
                return Err(tool_err(
                    "VALIDATION_ERROR",
                    "Invalid `watermark.position` (expected bottom_right|bottom_left|top_right|top_left|center).",
                    None,
                ));
            }
        },
    };
    Ok(TextWatermark {
        enabled,
        text,
        opacity,
        scale,
        position,
    })
}

fn parse_mcp_export_recipes(
    val: &Value,
    base_opts: &ExportOptions,
    naming_template: Option<&str>,
) -> Result<Vec<ExportRecipeSpec>, Value> {
    let Some(arr) = val.as_array() else {
        return Err(tool_err(
            "VALIDATION_ERROR",
            "`recipes` must be an array of recipe slugs (`full_res_jpeg`, `web_2048px`, `archival_16_bit_tiff`) or `ExportRecipeSpec` objects.",
            None,
        ));
    };
    if arr.is_empty() {
        return Err(tool_err(
            "VALIDATION_ERROR",
            "`recipes` array cannot be empty.",
            None,
        ));
    }
    let builtins = builtin_export_recipes();
    let mut specs = Vec::with_capacity(arr.len());
    for item in arr {
        match item {
            Value::String(s) => {
                let norm = s.trim().to_ascii_lowercase().replace(['-', ' '], "_");
                match norm.as_str() {
                    "full_res_jpeg" | "full" | "jpeg" | "jpg" => {
                        let mut r = builtins[0].clone();
                        r.enabled = true;
                        specs.push(r);
                    }
                    "web_2048px" | "web" | "web2048" => {
                        let mut r = builtins[1].clone();
                        r.enabled = true;
                        specs.push(r);
                    }
                    "archival_16_bit_tiff" | "archival" | "tiff" | "tiff16" => {
                        let mut r = builtins[2].clone();
                        r.enabled = true;
                        specs.push(r);
                    }
                    other => {
                        return Err(tool_err(
                            "VALIDATION_ERROR",
                            format!("Unknown recipe slug '{other}' in `recipes`."),
                            Some("Valid: full_res_jpeg, web_2048px, archival_16_bit_tiff"),
                        ));
                    }
                }
            }
            Value::Object(_) => {
                let spec: ExportRecipeSpec = serde_json::from_value(item.clone()).map_err(|e| {
                    tool_err(
                        "VALIDATION_ERROR",
                        format!("Invalid `ExportRecipeSpec` in `recipes`: {e}"),
                        None,
                    )
                })?;
                specs.push(spec);
            }
            _ => {
                return Err(tool_err(
                    "VALIDATION_ERROR",
                    "Each entry in `recipes` must be a recipe slug string or `ExportRecipeSpec` object.",
                    None,
                ));
            }
        }
    }
    for r in &mut specs {
        if let Some(tpl) = naming_template {
            r.naming_template = tpl.to_owned();
        }
        if base_opts.watermark.is_active() && !r.options.watermark.is_active() {
            r.options.watermark = base_opts.watermark.clone();
        }
        if base_opts.strip_metadata {
            r.options.strip_metadata = true;
        }
        if base_opts.color_space != ExportColorSpace::Srgb
            && r.options.color_space == ExportColorSpace::Srgb
        {
            r.options.color_space = base_opts.color_space;
        }
    }
    Ok(specs)
}

/// Validates an MCP output path (`out`) against prompt-injection file clobbering:
/// - Rejects directories, dotfiles without image extensions, RAW extensions, and non-image extensions.
/// - Delegates source-overwrite checks to shared `rune_editor_decode::guard_not_source`
///   (canonicalizing ancestors, symlinks, and lexical `..` traversal through non-existent dirs).
/// - Rejects overwriting any existing file/symlink unless `overwrite: true` is explicitly provided.
fn validate_mcp_output_path(
    dest: &Path,
    src_opt: Option<&Path>,
    overwrite: bool,
) -> Result<(), Value> {
    if dest.file_name().is_none() || dest.is_dir() {
        return Err(tool_err(
            "VALIDATION_ERROR",
            format!("Invalid output file path '{}'.", dest.display()),
            Some(
                "Provide a destination file path ending in .jpg, .jpeg, .png, .tif, .tiff, or .webp.",
            ),
        ));
    }

    if is_raw_extension(dest) {
        return Err(tool_err(
            "VALIDATION_ERROR",
            format!(
                "Refusing to write to RAW/camera source extension '{}'.",
                dest.display()
            ),
            Some("Use a developed image extension: .jpg, .jpeg, .png, .tif, .tiff, or .webp."),
        ));
    }

    let ext_lower = dest
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    if !ext_lower
        .as_deref()
        .is_some_and(|e| ALLOWED_MCP_OUTPUT_EXTENSIONS.contains(&e))
    {
        return Err(tool_err(
            "VALIDATION_ERROR",
            format!(
                "Disallowed output extension for '{}': only image outputs (.jpg, .jpeg, .png, .tif, .tiff, .webp) are permitted.",
                dest.display()
            ),
            Some("Pass an image path ending in .jpg, .jpeg, .png, .tif, .tiff, or .webp."),
        ));
    }

    if let Some(src) = src_opt
        && let Err(e) = guard_not_source(src, dest)
    {
        return Err(tool_err(
            "VALIDATION_ERROR",
            e.to_string(),
            Some("Export to a different filename or output directory."),
        ));
    }

    if !overwrite && (dest.exists() || dest.symlink_metadata().is_ok()) {
        return Err(tool_err(
            "FILE_EXISTS",
            format!(
                "Refusing to overwrite existing file '{}' without `overwrite: true`.",
                dest.display()
            ),
            Some("Choose a new output filename or explicitly pass `\"overwrite\": true`."),
        ));
    }

    Ok(())
}

fn parse_norm_xy(val: Option<&Value>, field: &str) -> Result<Option<[f32; 2]>, Value> {
    let Some(v) = val else {
        return Ok(None);
    };
    let Some(arr) = v.as_array() else {
        return Err(tool_err(
            "VALIDATION_ERROR",
            format!(
                "`{field}` must be a 2-element array `[x, y]` with normalized coordinates in 0.0..=1.0."
            ),
            Some(&format!("Example: `\"{field}\": [0.5, 0.5]`")),
        ));
    };
    if arr.len() != 2 {
        return Err(tool_err(
            "VALIDATION_ERROR",
            format!(
                "`{field}` requires exactly 2 coordinates `[x, y]` (got {}).",
                arr.len()
            ),
            Some(&format!("Example: `\"{field}\": [0.5, 0.5]`")),
        ));
    }
    let (Some(x), Some(y)) = (arr[0].as_f64(), arr[1].as_f64()) else {
        return Err(tool_err(
            "VALIDATION_ERROR",
            format!("`{field}` coordinates must be numeric `[x, y]` in 0.0..=1.0."),
            None,
        ));
    };
    let (xf, yf) = (x as f32, y as f32);
    if !xf.is_finite()
        || !yf.is_finite()
        || !(0.0..=1.0).contains(&xf)
        || !(0.0..=1.0).contains(&yf)
    {
        return Err(tool_err(
            "VALIDATION_ERROR",
            format!("`{field}` coordinates must be within 0.0..=1.0 (got [{xf}, {yf}])."),
            None,
        ));
    }
    Ok(Some([xf, yf]))
}

fn parse_color_slice_pick(val: Option<&Value>) -> Result<Option<(usize, [f32; 2])>, Value> {
    let Some(v) = val else {
        return Ok(None);
    };
    let Some(obj) = v.as_object() else {
        return Err(tool_err(
            "VALIDATION_ERROR",
            "`color_slice_pick` must be an object `{ \"index\": 0..=3, \"x\": 0..=1, \"y\": 0..=1 }`.",
            Some("Example: `\"color_slice_pick\": {\"index\": 0, \"x\": 0.5, \"y\": 0.4}`"),
        ));
    };
    for key in obj.keys() {
        if !matches!(key.as_str(), "index" | "x" | "y") {
            return Err(tool_err(
                "VALIDATION_ERROR",
                format!("Unknown key '{key}' in `color_slice_pick`."),
                Some("Allowed keys: `index` (0..=3), `x` (0..=1), `y` (0..=1)."),
            ));
        }
    }
    let Some(idx_u64) = obj.get("index").and_then(Value::as_u64) else {
        return Err(tool_err(
            "VALIDATION_ERROR",
            format!(
                "`color_slice_pick.index` must be an integer in 0..={}.",
                ADVANCED_COLOR_SLICE_COUNT - 1
            ),
            None,
        ));
    };
    let idx = idx_u64 as usize;
    if idx >= ADVANCED_COLOR_SLICE_COUNT {
        return Err(tool_err(
            "VALIDATION_ERROR",
            format!(
                "`color_slice_pick.index` ({idx}) out of range (expected 0..={}).",
                ADVANCED_COLOR_SLICE_COUNT - 1
            ),
            None,
        ));
    }
    let (Some(x), Some(y)) = (
        obj.get("x").and_then(Value::as_f64),
        obj.get("y").and_then(Value::as_f64),
    ) else {
        return Err(tool_err(
            "VALIDATION_ERROR",
            "`color_slice_pick.x` and `color_slice_pick.y` must be numbers in 0.0..=1.0.",
            None,
        ));
    };
    let (xf, yf) = (x as f32, y as f32);
    if !xf.is_finite()
        || !yf.is_finite()
        || !(0.0..=1.0).contains(&xf)
        || !(0.0..=1.0).contains(&yf)
    {
        return Err(tool_err(
            "VALIDATION_ERROR",
            format!("`color_slice_pick` coordinates must be within 0.0..=1.0 (got [{xf}, {yf}])."),
            None,
        ));
    }
    Ok(Some((idx, [xf, yf])))
}

fn sample_patch_3x3(img: &LinearImage, nx: f32, ny: f32) -> [f32; 3] {
    let w = img.width.max(1) as i32;
    let h = img.height.max(1) as i32;
    let cx = ((nx.clamp(0.0, 1.0) * (w - 1) as f32).round() as i32).clamp(0, w - 1);
    let cy = ((ny.clamp(0.0, 1.0) * (h - 1) as f32).round() as i32).clamp(0, h - 1);
    let mut sum = [0.0_f32; 3];
    let mut cnt = 0.0_f32;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let x = (cx + dx).clamp(0, w - 1) as usize;
            let y = (cy + dy).clamp(0, h - 1) as usize;
            let base = (y * img.width as usize + x) * 4;
            if base + 2 < img.rgba16f.len() {
                sum[0] += img.rgba16f[base].to_f32();
                sum[1] += img.rgba16f[base + 1].to_f32();
                sum[2] += img.rgba16f[base + 2].to_f32();
                cnt += 1.0;
            }
        }
    }
    if cnt > 0.0 {
        [sum[0] / cnt, sum[1] / cnt, sum[2] / cnt]
    } else {
        [0.18, 0.18, 0.18]
    }
}

fn parse_diff_assertions(val: Option<&Value>) -> Result<Vec<(String, f32)>, Value> {
    const VALID_METRICS: &[&str] = &[
        "max_highlight_clip_pct",
        "max_shadow_clip_pct",
        "max_skin_de2000",
        "max_mean_de2000",
    ];
    let Some(v) = val else {
        return Ok(Vec::new());
    };
    let Some(obj) = v.as_object() else {
        return Err(tool_err(
            "VALIDATION_ERROR",
            "`assertions` must be an object mapping metric names to numeric thresholds.",
            Some(
                "Valid keys: max_highlight_clip_pct, max_shadow_clip_pct, max_skin_de2000, max_mean_de2000",
            ),
        ));
    };
    let mut rules = Vec::with_capacity(obj.len());
    for (k, raw_limit) in obj {
        if !VALID_METRICS.contains(&k.as_str()) {
            return Err(tool_err(
                "VALIDATION_ERROR",
                format!("Unknown diff assertion metric '{k}'."),
                Some(
                    "Valid keys: max_highlight_clip_pct, max_shadow_clip_pct, max_skin_de2000, max_mean_de2000",
                ),
            ));
        }
        let Some(limit_f64) = raw_limit.as_f64() else {
            return Err(tool_err(
                "VALIDATION_ERROR",
                format!("Assertion threshold for '{k}' must be a number."),
                None,
            ));
        };
        let limit = limit_f64 as f32;
        if !limit.is_finite() || limit < 0.0 {
            return Err(tool_err(
                "VALIDATION_ERROR",
                format!("Assertion threshold for '{k}' must be a finite non-negative number."),
                None,
            ));
        }
        rules.push((k.clone(), limit));
    }
    Ok(rules)
}

/// Recursively applies a JSON `set` map onto `recipe`, supporting flat dot-paths,
/// nested group objects (`{"wheels":{"shadows":{"hue_x":-0.2}}}`), array values
/// (`curves.luma: [[0,0],[0.25,0.2],[1,1]]`), and keyed curve maps.
fn apply_set_map(
    recipe: &mut DevelopRecipe,
    prefix: &str,
    map: &serde_json::Map<String, Value>,
    src_dims: Option<[u32; 2]>,
) -> Result<(), Value> {
    for (k, v) in map {
        let full_key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        let canonical = rune_editor_core::canonical_param_key(&full_key);
        let is_curve_leaf = matches!(
            canonical.as_str(),
            "curves.luma"
                | "curves.master"
                | "curves.rgb"
                | "curves.red"
                | "curves.green"
                | "curves.blue"
        );

        match v {
            Value::Object(nested) if !is_curve_leaf => {
                apply_set_map(recipe, &full_key, nested, src_dims)?;
            }
            Value::Object(_) | Value::Array(_) => {
                let raw = serde_json::to_string(v).unwrap_or_default();
                set_param_by_str(recipe, &full_key, &raw, false, src_dims)
                    .map_err(|e| tool_err("VALIDATION_ERROR", e.to_string(), None))?;
            }
            Value::String(s) => {
                let (val_str, delta) = if let Some(rest) = s.strip_prefix("+=") {
                    (rest.to_owned(), true)
                } else if let Some(rest) = s.strip_prefix("-=") {
                    (format!("-{rest}"), true)
                } else {
                    (s.clone(), false)
                };
                set_param_by_str(recipe, &full_key, &val_str, delta, src_dims)
                    .map_err(|e| tool_err("VALIDATION_ERROR", e.to_string(), None))?;
            }
            Value::Number(n) => {
                set_param_by_str(recipe, &full_key, &n.to_string(), false, src_dims)
                    .map_err(|e| tool_err("VALIDATION_ERROR", e.to_string(), None))?;
            }
            Value::Bool(b) => {
                set_param_by_str(recipe, &full_key, &b.to_string(), false, src_dims)
                    .map_err(|e| tool_err("VALIDATION_ERROR", e.to_string(), None))?;
            }
            Value::Null => {
                return Err(tool_err(
                    "VALIDATION_ERROR",
                    format!("Null value is not valid for set.{full_key}"),
                    None,
                ));
            }
        }
    }
    Ok(())
}

/// Complete MCP 2025-11-25 tool catalog with annotations & output schemas.
pub fn tool_definitions() -> Vec<ToolDef> {
    let presets: Vec<&str> = builtin_presets().iter().map(|p| p.id).collect();
    let wb_presets: Vec<&str> = WbPreset::ALL.iter().map(|p| p.cli_slug()).collect();
    let film_curves: Vec<&str> = FilmCurve::ALL.iter().map(|c| c.cli_slug()).collect();
    let processing_modes: Vec<&str> = ProcessingMode::ALL.iter().map(|m| m.cli_slug()).collect();
    let vignette_methods: Vec<&str> = VignetteMethod::ALL.iter().map(|m| m.cli_slug()).collect();
    vec![
        ToolDef {
            name: "rune_get_schema",
            title: "Get Develop Schema & Active Session",
            description: "Returns all registered Rune Darkroom develop parameters with exact min/max bounds, defaults, built-in presets, and the currently open GUI session state.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {
                    "schema": { "type": "object" },
                    "active_session": { "type": "object" }
                }
            }),
            annotations: ToolAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
                open_world_hint: false,
            },
        },
        ToolDef {
            name: "rune_inspect_photo",
            title: "Inspect Photo Metadata & Sidecar",
            description: "Fast read-only inspection of camera EXIF metadata, `camera_profile` & `profile_strength`, Kelvin white balance readout (`wb_kelvin`, `wb_preset`), `film_curve`, `processing_mode` (plus `film_negative` when active), rating, cull flag, `color_tag`, virtual variant summary, layer/heal counts, and non-default recipe overrides. Omit `path` to inspect the photo currently open in Rune Darkroom.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Optional path to photo file. Omit to target the live GUI canvas." }
                }
            }),
            output_schema: json!({ "type": "object" }),
            annotations: ToolAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
                open_world_hint: false,
            },
        },
        ToolDef {
            name: "rune_analyze_photo",
            title: "Analyze Photo Tone, Color & Focus",
            description: "Computes quantitative perceptual metrics: dynamic range, highlight/shadow clipping %, Zone 0-X distribution, neutral & tonal CIE Lab color cast, skin-tone guard, 5-color palette, sharpness score, and auto WB/tone/levels suggestions.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Optional photo path; omit to analyze the live GUI canvas." },
                    "proxy_edge": { "type": "integer", "minimum": 64, "maximum": 1600, "default": 320 },
                    "fast": { "type": "boolean", "default": false, "description": "Skip 1024px sharpness analysis for faster triage." }
                }
            }),
            output_schema: json!({ "type": "object" }),
            annotations: ToolAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
                open_world_hint: false,
            },
        },
        ToolDef {
            name: "rune_edit_photo",
            title: "Edit Photo Non-Destructively",
            description: "Non-destructively edits a photo's `.rune-edit.json` sidecar (and live-updates the open Rune Darkroom viewport with Cmd+Z undo support). Supports built-in presets with blend `mix`, `auto` (`wb`/`tone`/`levels`/`all`), `camera_profile` & `profile_strength` (applied before `patch`/`set`), `wb_pick` (`[x, y]` eyedropper), `wb_preset`, `film_curve`, `processing_mode` (`photography`/`film_negative`), `film_base_pick` (`[x, y]` raw film border eyedropper), `vignette_method`, `skin_pick` (`[x, y]`), `color_slice_pick` (`{index, x, y}`), typed sparse `patch`, dot-path `set` map, and `reset`.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Optional photo path; omit to edit the live GUI canvas." },
                    "preset": { "type": "string", "enum": presets },
                    "mix": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 1.0 },
                    "auto": { "type": "array", "items": { "type": "string", "enum": ["wb", "tone", "levels", "all"] } },
                    "camera_profile": {
                        "type": "string",
                        "description": "Camera calibration or film simulation profile ID (`auto`, `pro_standard`, `camera_generic`, `apple_raw`, `linear_scientific`, `portrait_natural`, `landscape_crisp`, `editorial_neutral`, `classic_chrome`, `vivid_velvia`, `soft_astia`, `acros_mono`, `warm_portra`, `tungsten_cine`, or `user:<slug>`). Applied before `patch` and `set`."
                    },
                    "profile_strength": {
                        "type": "number",
                        "minimum": 0.0,
                        "maximum": 1.0,
                        "default": 1.0,
                        "description": "Blend strength for the active camera profile (`0.0..=1.0`, default `1.0`). Applied before `patch` and `set`."
                    },
                    "wb_pick": {
                        "type": "array",
                        "items": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                        "minItems": 2,
                        "maxItems": 2,
                        "description": "Normalized `[x, y]` coordinate in `0.0..=1.0` of a neutral reference patch for eyedropper white balance."
                    },
                    "wb_preset": {
                        "type": "string",
                        "enum": wb_presets,
                        "description": "Standard illuminant white balance preset (updates temperature and tint for standard presets)."
                    },
                    "film_curve": {
                        "type": "string",
                        "enum": film_curves,
                        "description": "Base Characteristics tone response film curve (`film_standard`, `film_extra_shadow`, `film_high_contrast`, `linear_response`)."
                    },
                    "processing_mode": {
                        "type": "string",
                        "enum": processing_modes,
                        "description": "Base Characteristics processing mode (`photography` or `film_negative`)."
                    },
                    "film_base_pick": {
                        "type": "array",
                        "items": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                        "minItems": 2,
                        "maxItems": 2,
                        "description": "Normalized `[x, y]` coordinate in `0.0..=1.0` of an unexposed film border to neutralize orange mask base RGB and enable Film Negative mode."
                    },
                    "vignette_method": {
                        "type": "string",
                        "enum": vignette_methods,
                        "description": "Crop-aware vignetting geometry method (`elliptic_on_crop`, `circular_on_crop`, `circular`)."
                    },
                    "skin_pick": {
                        "type": "array",
                        "items": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                        "minItems": 2,
                        "maxItems": 2,
                        "description": "Normalized `[x, y]` coordinate in `0.0..=1.0` to sample Skin Tone Uniformity target H/S/L and enable `skin_tone`."
                    },
                    "color_slice_pick": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["index", "x", "y"],
                        "properties": {
                            "index": { "type": "integer", "minimum": 0, "maximum": 3 },
                            "x": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                            "y": { "type": "number", "minimum": 0.0, "maximum": 1.0 }
                        },
                        "description": "Sample linear pixel at normalized `(x, y)` to set target Hue/Sat and enable Advanced Color Editor slice `0..=3`."
                    },
                    "patch": {
                        "type": "object",
                        "description": "Typed sparse RecipePatch supporting scalars (bare number or `\"+0.25\"` relative string), `camera_profile` (`auto`, `pro_standard`, `classic_chrome`, `user:<slug>`, etc.), `profile_strength` (`0..=1`), `processing_mode`, `film_negative` (`base_r`/`base_g`/`base_b`/`density`/`contrast`/`black_clip`/`white_clip`), `film_curve`, `wb_preset`, `clarity_method`, `vignette_method`, `sharpen_radius`/`sharpen_threshold`/`sharpen_halo`, `nr_color`, `grain_type`, `grain_size`, `shadows_wheel`/`midtones_wheel`/`highlights_wheel`/`master_wheel` (or grouped `wheels:{shadows,midtones,highlights,master}`), `hsl:{red..magenta}`, `advanced_color:{\"0\"..\"3\":{...}}`, `skin_tone`, `levels` (master + `red`/`green`/`blue`), `curves:{rgb,luma,red,green,blue}` (bare `[[x,y],...]`, flat array, keyed map, or `\"reset\"`), `bw` (incl. `split_*`), `lens` (`distortion`, `chromatic_aberration`, `purple_fringing`, `light_falloff`), and `geometry` (incl. `keystone_vertical`, `keystone_horizontal`, `keystone_aspect`, `keystone_amount`)."
                    },
                    "set": {
                        "type": "object",
                        "description": "Dot-path or nested parameter assignments e.g. `{\"camera_profile\":\"classic_chrome\", \"profile_strength\":0.8, \"processing_mode\":\"film_negative\", \"film_negative.density\":0.2, \"film_curve\":\"film_extra_shadow\", \"wb_preset\":\"daylight\", \"vignette_method\":\"circular_on_crop\", \"advanced_color.0.uniformity\":0.5, \"skin_tone.uniformity_hue\":0.6, \"lens.distortion\":-0.15, \"geometry.keystone_vertical\":0.2, \"levels.red.in_black\":0.04, \"wheels.master_wheel.hue_x\":0.1, \"curves.rgb\":[[0,0],[0.5,0.54],[1,1]], \"exposure\":\"+=0.2\"}`."
                    },
                    "reset": { "type": "string", "description": "Optional tool group to reset before applying edits (`all`, `tone`, `film_negative`, `wb`, `color`, `wheels`, `hsl`, `advanced_color`, `skin_tone`, `levels`, `curves`, `detail`, `bw`, `effects`, `lens`, `geometry`, `layers`)." },
                    "agent": { "type": "string", "default": "mcp-agent" },
                    "dry_run": { "type": "boolean", "default": false }
                }
            }),
            output_schema: json!({ "type": "object" }),
            annotations: ToolAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
                open_world_hint: false,
            },
        },
        ToolDef {
            name: "rune_preview_photo",
            title: "Render Inline Visual Preview",
            description: "Develops the photo with full spatial detail, vignette, grain, and geometry parity and returns an inline base64 JPEG plus clipping telemetry. Optional `out` writes only to image extensions (.jpg/.jpeg/.png/.tif/.tiff/.webp), refuses to overwrite the source photo, and requires `overwrite: true` to replace an existing file.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Optional photo path; omit to preview the live GUI canvas." },
                    "out": { "type": "string", "description": "Optional image file path (.jpg/.jpeg/.png/.tif/.tiff/.webp) to save the rendered preview JPEG. Refuses overwriting existing files unless `overwrite: true`." },
                    "overwrite": { "type": "boolean", "default": false, "description": "Must be explicitly `true` to overwrite an existing `out` file (never permits overwriting the original source photo)." },
                    "max_edge": { "type": "integer", "minimum": 128, "maximum": 1600, "default": 1024 },
                    "quality": { "type": "integer", "minimum": 30, "maximum": 96, "default": 82 }
                }
            }),
            output_schema: json!({ "type": "object" }),
            annotations: ToolAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: true,
                open_world_hint: false,
            },
        },
        ToolDef {
            name: "rune_diff_photo",
            title: "Compute CIEDE2000 Perceptual Diff",
            description: "Computes exact CIEDE2000 color difference (mean, p95, max, shadows, midtones, highlights, skin_tones), EV luma shift, clipping deltas, and field changes against neutral or a preset, with optional quantitative guardrail `assertions`.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Optional photo path; omit for live GUI canvas." },
                    "against_preset": { "type": "string", "enum": presets },
                    "assertions": {
                        "type": "object",
                        "additionalProperties": false,
                        "description": "Optional guardrail thresholds that fail the call (`assertions_passed: false`, `isError: true`) when exceeded.",
                        "properties": {
                            "max_highlight_clip_pct": { "type": "number", "minimum": 0.0 },
                            "max_shadow_clip_pct": { "type": "number", "minimum": 0.0 },
                            "max_skin_de2000": { "type": "number", "minimum": 0.0 },
                            "max_mean_de2000": { "type": "number", "minimum": 0.0 }
                        }
                    }
                }
            }),
            output_schema: json!({ "type": "object" }),
            annotations: ToolAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
                open_world_hint: false,
            },
        },
        ToolDef {
            name: "rune_cull_or_export",
            title: "Cull Metadata or Export Developed Photo",
            description: "Updates star ratings, Pick/Reject flags, and 7 Color Tags (`action: \"cull\"`) or exports a developed photo (`action: \"export\"`) with optional dynamic `naming_template`, text `watermark`, `strip_metadata`, `color_space` (`srgb`/`display_p3`), or simultaneous Multi-Recipe `recipes` without ever overwriting originals (requires `overwrite: true` to replace existing files).",
            input_schema: json!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": { "type": "string", "enum": ["cull", "export"] },
                    "path": { "type": "string", "description": "Optional source photo path; omit for live GUI canvas." },
                    "rating": { "type": "integer", "minimum": 0, "maximum": 5 },
                    "flag": { "type": "string", "enum": ["pick", "reject", "none"] },
                    "color_tag": { "type": "string", "enum": ["none", "red", "orange", "yellow", "green", "blue", "purple", "pink"] },
                    "out": { "type": "string", "description": "Destination image file path (.jpg/.jpeg/.png/.tif/.tiff/.webp) or output directory when `recipes` / `naming_template` is set." },
                    "overwrite": { "type": "boolean", "default": false, "description": "Must be explicitly `true` to overwrite an existing `out` file (never permits overwriting the original source photo)." },
                    "format": { "type": "string", "enum": ["jpeg", "png8", "png16", "tiff16", "webp"], "default": "jpeg" },
                    "quality": { "type": "integer", "minimum": 1, "maximum": 100, "default": 92 },
                    "max_edge": { "type": "integer", "minimum": 64 },
                    "naming_template": { "type": "string", "description": "Capture One dynamic naming template e.g. `{ImageName}_{Recipe}_{Variant}`." },
                    "watermark": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "enabled": { "type": "boolean" },
                            "text": { "type": "string" },
                            "opacity": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.45 },
                            "scale": { "type": "number", "minimum": 0.5, "maximum": 3.0, "default": 1.0 },
                            "position": { "type": "string", "enum": ["bottom_right", "bottom_left", "top_right", "top_left", "center"], "default": "bottom_right" }
                        }
                    },
                    "strip_metadata": { "type": "boolean", "default": false },
                    "color_space": { "type": "string", "enum": ["srgb", "display_p3", "display-p3"], "default": "srgb" },
                    "recipes": {
                        "type": "array",
                        "description": "Simultaneous Multi-Recipe Export list of recipe slugs (`full_res_jpeg`, `web_2048px`, `archival_16_bit_tiff`) or full `ExportRecipeSpec` objects."
                    }
                }
            }),
            output_schema: json!({ "type": "object" }),
            annotations: ToolAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
                open_world_hint: false,
            },
        },
        ToolDef {
            name: "rune_manage_workflow",
            title: "Manage Virtual Variants, User Styles, Camera Profiles & Selective Clipboard",
            description: "Capture One Pro workflow operations: create/clone/select/delete non-destructive Virtual Variants (`variant_*`), save/list/apply/stack/delete reusable User Styles (`style_*`), copy selective or auto-detected adjusted tool groups across photos (`copy_selective_groups`), initialize 4-folder sessions (`session_init`), and list bundled + imported Camera Profiles (`profile_list` with optional EXIF `camera_make`/`camera_model` matching).",
            input_schema: json!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": [
                            "variant_list",
                            "variant_new",
                            "variant_clone",
                            "variant_select",
                            "variant_delete",
                            "style_list",
                            "style_save",
                            "style_apply",
                            "style_delete",
                            "copy_selective_groups",
                            "session_init",
                            "profile_list"
                        ]
                    },
                    "path": { "type": "string", "description": "Target photo path (optional when connected to a disk photo in the live GUI)." },
                    "dir": { "type": "string", "description": "Root session directory path for `session_init` (creates Capture/Selects/Output/Trash; rejects `..` traversal and symlinks)." },
                    "src_path": { "type": "string", "description": "Source photo path for `copy_selective_groups`." },
                    "dst_path": { "type": "string", "description": "Destination photo path for `copy_selective_groups` (omit for live GUI canvas)." },
                    "label": { "type": "string", "description": "Optional label for `variant_new` / `variant_clone`." },
                    "index": { "type": "integer", "minimum": 0, "description": "Zero-based variant index for `variant_select` / `variant_delete`." },
                    "name": { "type": "string", "description": "User Style name/slug for `style_save`, `style_apply`, `style_delete`." },
                    "description": { "type": "string", "description": "Optional description for `style_save`." },
                    "groups": { "type": "array", "items": { "type": "string" }, "description": "Selective tool groups (`wb`, `tone`, `film_negative`, `levels`, `curves`, `wheels`, `color`, `skin_tone`, `bw`, `detail`, `effects`, `lens`, `geometry`, `layers`, `heal`)." },
                    "autoselect_adjusted": { "type": "boolean", "default": false, "description": "Automatically select only tool groups modified from defaults." },
                    "stack": { "type": "boolean", "default": false, "description": "Additively stack User Style onto existing adjustments instead of replacing." },
                    "strength": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 1.0 },
                    "camera_make": { "type": "string", "description": "Optional EXIF camera make for `profile_list` (e.g. `Sony`, `Fujifilm`, `Apple`) to highlight camera-matched profiles." },
                    "camera_model": { "type": "string", "description": "Optional EXIF camera model for `profile_list` (e.g. `ILCE-7RM5`, `X-T5`, `iPhone 16 Pro`) to sort camera-matched user profiles first." }
                }
            }),
            output_schema: json!({ "type": "object" }),
            annotations: ToolAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
                open_world_hint: false,
            },
        },
        ToolDef {
            name: "rune_edit_layers",
            title: "Edit Adjustment Layers, Combine Masks, AI Subject & Heal/Clone",
            description: "Non-destructively manages up to 8 Capture One-style Adjustment Layers, Composable Masks (`add`/`subtract`/`intersect` for Filled, Linear, Radial, Luma Range, Brush, AI Subject, AI Background), Local Adjustments, and up to 16 Spot Heal & Clone stamps (live-updates the open GUI canvas with Cmd+Z undo).",
            input_schema: json!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": [
                            "layer_list",
                            "layer_add",
                            "layer_update",
                            "layer_remove",
                            "mask_add",
                            "mask_clear",
                            "ai_subject",
                            "ai_background",
                            "heal_list",
                            "heal_add",
                            "heal_remove",
                            "heal_clear"
                        ]
                    },
                    "path": { "type": "string", "description": "Optional photo path; omit to target the live GUI canvas." },
                    "index": { "type": "integer", "minimum": 0, "description": "Zero-based layer or heal spot index." },
                    "id": { "type": "string", "description": "Layer or heal spot ID (alternative to `index`)." },
                    "name": { "type": "string", "description": "Layer display name." },
                    "kind": { "type": "string", "enum": ["empty", "filled", "linear", "radial", "luma", "brush", "ai_subject", "ai_background"] },
                    "opacity": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                    "enabled": { "type": "boolean" },
                    "inverted": { "type": "boolean" },
                    "op": { "type": "string", "enum": ["add", "subtract", "intersect"], "default": "add" },
                    "adjustments": { "type": "object", "description": "Local slider map (`exposure`, `contrast`, `brightness`, `highlights`, `shadows`, `temperature`, `tint`, `saturation`, `clarity`, `dehaze`, `sharpen`)." },
                    "start": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 },
                    "end": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 },
                    "center": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 },
                    "radius": { "description": "Radial semi-axes `[rx, ry]` or scalar radius (also Spot Heal/Clone radius in `0.002..=0.25`)." },
                    "angle_deg": { "type": "number", "minimum": -180.0, "maximum": 180.0 },
                    "feather": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                    "low": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                    "high": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                    "low_falloff": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                    "high_falloff": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                    "strokes": { "type": "array", "description": "Array of `{x, y, radius, hardness, opacity, erase}` brush dabs." },
                    "target": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "Normalized `[x, y]` target coordinate for `heal_add`." },
                    "source": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "Optional normalized `[x, y]` source patch coordinate for `heal_add`." },
                    "mode": { "type": "string", "enum": ["heal", "clone"], "default": "heal" },
                    "agent": { "type": "string", "default": "mcp-layers" },
                    "dry_run": { "type": "boolean", "default": false }
                }
            }),
            output_schema: json!({ "type": "object" }),
            annotations: ToolAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
                open_world_hint: false,
            },
        },
    ]
}
