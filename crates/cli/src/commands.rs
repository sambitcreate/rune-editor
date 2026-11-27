//! Subcommand implementations for `rd` (Rune Darkroom CLI).

use std::collections::HashMap;
use std::io::{self, BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::time::Instant;

use clap::CommandFactory;
use rayon::prelude::*;
use rune_editor_core::{
    ADVANCED_COLOR_SLICE_COUNT, ALLOWED_PROFILE_EXTENSIONS, AdjustmentLayer, BatchJob,
    CameraProfileDescriptor, ColorTag, CullFlag, DevelopRecipe, EditActor, EditOp, HealMode,
    HealSpot, LayerMaskItem, LocalAdjustments, MAX_ADJUSTMENT_LAYERS, MAX_HEAL_SPOTS,
    MAX_MASKS_PER_LAYER, MAX_PROFILE_FILE_BYTES, MAX_PROMPT_POINTS, MaskCombineOp, MaskShape,
    ModelId, ModelInstallState, PhotoMeta, ProcessingMode, ProfileCategory, PromptBox, PromptPoint,
    RecipePatch, SegmentationEngine, SmartMaskPrompt, USER_PROFILE_PREFIX, UserStyle, WbPreset,
    analyze_linear_proxy, apply_style_to_recipe, auto_exposure_and_tone, auto_levels,
    auto_white_balance, blend_recipes, builtin_camera_profiles, builtin_presets,
    canonical_param_key, compute_histogram, copy_recipe_groups, create_capture_one_session,
    default_profiles_dir, default_styles_dir, delete_user_profile, delete_user_profile_in_dir,
    delete_user_style_in_dir, diff_recipes, filmic_tonemap_with_curve, import_user_profile,
    import_user_profile_in_dir_with_overwrite, inspect_model_status, linear_to_srgb,
    list_all_camera_profiles, list_user_styles_in_dir, load_ai_model_config,
    load_user_style_in_dir, modified_recipe_groups, parse_cube_lut, parse_dcp_profile,
    parse_icc_profile, pick_white_balance, recipe_field_diffs, recipe_non_default_map,
    recipe_schema_json, reset_recipe_group, resolve_builtin_preset, rgb_to_hsl_and_lab,
    sanitize_profile_slug, save_user_style_in_dir, search_params, set_param_by_str,
    stack_style_onto_recipe, temperature_to_kelvin, update_ai_model_config,
};
use rune_editor_decode::{
    ExportColorSpace, ExportFormat, ExportOptions, ExportRecipeSpec, HotFolderWatcher,
    NamingContext, RASTER_EXTENSIONS, RAW_EXTENSIONS, ScanOptions, SmartMaskResult, TextWatermark,
    WatermarkPosition, active_engine_summary, builtin_export_recipes, download_model,
    expand_naming_template, export_image_with_options, export_multi_recipe,
    extract_detail_luma_proxy, extract_exif_from_path, generate_smart_mask,
    guard_not_source as decode_guard_not_source, is_raw_extension, is_supported_photo,
    list_model_statuses, load_file, load_file_proxy, plan_multi_recipe_paths,
    probe_image_dimensions, remove_model, sample_linear_rgb_at_uv, sample_raw_linear_rgb_at_uv,
    scan_photo_folder_with_options, setup_model_from_local_dir,
};
use serde_json::{Value, json};

use crate::{
    AnalyzeArgs, BatchCmd, Cli, CliError, CliSearchCmd, Command, CopyLookArgs, CullArgs, DiffArgs,
    EditArgs, ExportArgs, HealCmd, InspectArgs, LayerCmd, ModelCmd, PresetCmd, PreviewArgs,
    ProfileCmd, ResolvedFormat, RuntimeOpts, SessionCmd, StyleCmd, VariantCmd, emit_ndjson_line,
    emit_ok, exit_code,
};

pub fn dispatch(command: Command, opts: &RuntimeOpts) -> Result<u8, CliError> {
    match command {
        Command::Schema { target } => cmd_schema(&target, opts),
        Command::Capabilities => cmd_capabilities(opts),
        Command::Cli { sub } => match sub {
            CliSearchCmd::Search { query } => cmd_cli_search(&query, opts),
        },
        Command::Inspect(args) => cmd_inspect(args, opts),
        Command::Analyze(args) => cmd_analyze(args, opts),
        Command::Edit(args) => cmd_edit(*args, opts),
        Command::Cull(args) => cmd_cull(args, opts),
        Command::Variant { sub } => cmd_variant(sub, opts),
        Command::Style { sub } => cmd_style(sub, opts),
        Command::CopyLook(args) => cmd_copy_look(args, opts),
        Command::Layer { sub } => cmd_layer(sub, opts),
        Command::Model { sub } => cmd_model(sub, opts),
        Command::Heal { sub } => cmd_heal(sub, opts),
        Command::Session { sub } => match sub {
            SessionCmd::Scan {
                dir,
                recursive,
                picks,
                rejects,
                min_rating,
                color_tag,
                search,
                ext,
            } => cmd_session_scan(
                &dir, recursive, picks, rejects, min_rating, color_tag, search, ext, opts,
            ),
            SessionCmd::Init { dir } => cmd_session_init(&dir, opts),
            SessionCmd::Watch {
                dir,
                style,
                once,
                interval_ms,
            } => cmd_session_watch(&dir, style.as_deref(), once, interval_ms, opts),
        },
        Command::Preset { sub } => cmd_preset(sub, opts),
        Command::Profile { sub } => cmd_profile(sub, opts),
        Command::Preview(args) => cmd_preview(args, opts),
        Command::Diff(args) => cmd_diff(args, opts),
        Command::Export(args) => cmd_export(args, opts),
        Command::Batch { sub } => match sub {
            BatchCmd::Run { manifest, jobs } => cmd_batch_run(&manifest, jobs, opts),
        },
        Command::Open { path } => cmd_open(path, opts),
        Command::Completions { shell } => {
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "rd", &mut io::stdout());
            Ok(exit_code::OK)
        }
        Command::Mcp { http, port } => cmd_mcp(http, port),
    }
}

fn cmd_schema(target: &str, opts: &RuntimeOpts) -> Result<u8, CliError> {
    let commands_meta = json!([
        { "command": "rd schema [target]", "summary": "Emit parameter bounds, enums, and JSON schemas" },
        { "command": "rd capabilities", "summary": "Report platform decode/export capabilities and presets" },
        { "command": "rd cli search <query>", "summary": "Find matching commands, recipe parameters, and presets" },
        { "command": "rd inspect <photo>", "summary": "Fast metadata, EXIF, and sidecar inspection" },
        { "command": "rd analyze <paths...>", "summary": "Quantitative tonal, color cast, skin, palette, sharpness, and auto suggestions" },
        { "command": "rd edit <paths...>", "summary": "Non-destructively update `.rune-edit.json` sidecars (`--set`, `--preset`, `--profile`, `--profile-strength`, `--wb-preset`, `--film-curve`, `--processing-mode`, `--pick-film-base`, `--vignette-method`, `--pick-skin`, `--pick-color-slice`, `--auto`, `--patch`, `--reset`)" },
        { "command": "rd cull <paths...>", "summary": "Batch-set star ratings, Pick/Reject flags, and 7 Color Tags (`--color-tag`)" },
        { "command": "rd variant list|new|clone|select|delete <photo>", "summary": "Manage non-destructive Virtual Variants inside `.rune-edit.json`" },
        { "command": "rd style list|save|apply|delete", "summary": "Save, apply, or stack reusable User Styles (`--autoselect-adjusted`, `--stack`, `--strength`)" },
        { "command": "rd copy-look <src> <dst...>", "summary": "Selective Adjustments Clipboard (`--groups` or `--autoselect-adjusted`)" },
        { "command": "rd layer list|add|update|mask-add|mask-clear|remove <photo>", "summary": "Manage Adjustment Layers, Combine Masks (`add`/`subtract`/`intersect`), Linear/Radial/Luma, AI Subject/Background, and interactive AI Select (`--kind ai-select --point --box --engine --invert`) masks" },
        { "command": "rd model list|status|setup|use|remove", "summary": "Manage AI segmentation engines (Apple Vision ANE built-in, SAM 2.1, SAM 3 ONNX): list, status, download/setup (`--accept-license`, `--from-dir`), select active engine, or remove weights" },
        { "command": "rd heal list|add|remove|clear <photo>", "summary": "Manage Spot Heal & Clone retouching stamps" },
        { "command": "rd session scan <dir>", "summary": "Scan and filter a photo directory (`--picks`, `--min-rating`, `--color-tag`, `--search`)" },
        { "command": "rd session init <dir>", "summary": "Initialize a Capture One 4-folder session (`Capture/`, `Selects/`, `Output/`, `Trash/`)" },
        { "command": "rd session watch <dir>", "summary": "Watch a hot folder for incoming photos (`--style`, `--once`, `--interval-ms`)" },
        { "command": "rd preset list|show|apply|save", "summary": "Manage built-in and custom style presets" },
        { "command": "rd profile list|import|delete", "summary": "List bundled Calibration/Film Simulation & user profiles (`--camera`, `--make`), import `.dcp`/`.icc`/`.icm`/`.cube` (`--name`, `--overwrite`), or delete user profiles" },
        { "command": "rd preview <photo> -o <out.jpg>", "summary": "Render fast vision-agent preview (default 1280px JPEG q=85)" },
        { "command": "rd diff <photo> [baseline]", "summary": "Compute CIEDE2000, EV shift, clipping delta, and `--assert` guardrails" },
        { "command": "rd export <paths...> -o <dest>", "summary": "Full-res, resized, watermarked, Display P3, or simultaneous Multi-Recipe export (`--recipes`, `--naming-template`, `--watermark`, `--color-space`)" },
        { "command": "rd batch run <job.json|->", "summary": "Execute declarative conditional batch rules + multi-target exports" },
        { "command": "rd open [path]", "summary": "Open photo or folder in Rune Darkroom GUI" }
    ]);

    let patch_meta = json!({
        "description": "Typed sparse patch (`deny_unknown_fields`). Scalars accept bare number `0.35`, relative string `\"+0.25\"` / `\"-0.15\"`, or `{\"add\": 0.25}` / `{\"scale\": 1.1}`. Supports `camera_profile` (`auto`, `pro_standard`, `classic_chrome`, `user:<slug>`, etc.), `profile_strength` (`0..=1`), `processing_mode`, `film_negative` (`base_r`/`base_g`/`base_b`/`density`/`contrast`/`black_clip`/`white_clip`), `film_curve`, `wb_preset`, `clarity_method`, `grain_type`, `vignette_method`, `sharpen_radius`/`threshold`/`halo`, `nr_color`, `grain_size`, `wheels` (incl. `master`), `curves` (`rgb`/`luma`/`red`/`green`/`blue`), `levels` (master + `red`/`green`/`blue`), `hsl`, `advanced_color` (`0`..`3`), `skin_tone`, `bw` (incl. `split_*`), `lens`, and `geometry` (incl. `keystone_*`).",
        "example": {
            "camera_profile": "pro_standard",
            "profile_strength": 0.85,
            "film_curve": "film_extra_shadow",
            "wb_preset": "daylight",
            "exposure": "+0.30",
            "highlights": -0.35,
            "clarity_method": "punch",
            "vignette_method": "circular_on_crop",
            "grain_type": "silver_rich",
            "curves": { "rgb": [[0, 0], [0.25, 0.22], [0.75, 0.78], [1, 1]] },
            "levels": { "red": { "in_black": 0.02 } },
            "wheels": { "master": { "hue_x": 0.05, "luma": 0.02 } },
            "hsl": { "orange": { "saturation": -0.15 } },
            "advanced_color": { "0": { "enabled": true, "target_hue": 210, "uniformity": 0.4 } },
            "skin_tone": { "enabled": true, "uniformity_hue": 0.5 },
            "lens": { "distortion": -0.15, "chromatic_aberration": 0.2, "purple_fringing": 0.3 },
            "geometry": { "aspect_preset": "FourFive", "keystone_vertical": 0.2 }
        }
    });

    let batch_meta = json!({
        "description": "Declarative BatchJob JSON manifest with conditional metadata/tonal rules and multi-target exports.",
        "required_fields": ["schema_version", "inputs"],
        "conditions": ["iso_gte", "iso_lte", "flag_is", "rating_gte", "highlight_clip_pct_gt", "shadow_clip_pct_gt", "all", "any", "not"],
        "sidecar_modes": ["update", "create_if_missing", "memory_only"]
    });

    let payload = match target.trim().to_ascii_lowercase().as_str() {
        "recipe" => recipe_schema_json(),
        "patch" => patch_meta,
        "batch" => batch_meta,
        "commands" => commands_meta,
        "all" => json!({
            "recipe": recipe_schema_json(),
            "patch": patch_meta,
            "batch": batch_meta,
            "commands": commands_meta,
            "exit_codes": {
                "0": "ok",
                "1": "partial_batch_failure_or_assert_failed",
                "2": "validation_or_usage_error",
                "3": "input_not_found",
                "4": "decode_error",
                "5": "io_or_sidecar_conflict",
                "6": "gpu_unavailable"
            }
        }),
        other => {
            return Err(CliError::validation(format!(
                "Unknown schema target '{other}'. Valid: recipe, patch, batch, commands, all"
            )));
        }
    };

    let human = serde_json::to_string_pretty(&payload).unwrap_or_default();
    emit_ok(opts, "schema", payload, &human);
    Ok(exit_code::OK)
}

fn cmd_capabilities(opts: &RuntimeOpts) -> Result<u8, CliError> {
    let presets: Vec<&str> = builtin_presets().iter().map(|p| p.id).collect();
    let profiles: Vec<String> = builtin_camera_profiles()
        .into_iter()
        .map(|p| p.id)
        .collect();
    let decode_tier = if cfg!(target_os = "macos") {
        "CIRAWFilter+ImageIO (macOS native) + rawler fallback"
    } else {
        "rawler + image-rs (cross-platform)"
    };
    let data = json!({
        "cli_version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "decode_tier": decode_tier,
        "export_backends": ["cpu"],
        "raw_extensions": RAW_EXTENSIONS,
        "raster_extensions": RASTER_EXTENSIONS,
        "export_formats": ["jpeg", "png8", "png16", "tiff16", "webp"],
        "builtin_presets": presets,
        "builtin_camera_profiles": profiles,
        "profile_import_extensions": ALLOWED_PROFILE_EXTENSIONS,
        "registered_params_count": rune_editor_core::all_param_descriptors().len(),
    });
    let human = format!(
        "Rune Darkroom CLI v{} ({}/{})\nDecode: {}\nFormats: jpeg, png8, png16, tiff16, webp\nPresets: {}\nCamera Profiles: {} ({} bundled; import .dcp/.icc/.icm/.cube)\nRegistered params: {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        decode_tier,
        presets.join(", "),
        profiles.join(", "),
        profiles.len(),
        rune_editor_core::all_param_descriptors().len(),
    );
    emit_ok(opts, "capabilities", data, &human);
    Ok(exit_code::OK)
}

fn cmd_cli_search(query: &str, opts: &RuntimeOpts) -> Result<u8, CliError> {
    let q = query.trim().to_ascii_lowercase();
    let params = search_params(&q);
    let presets: Vec<Value> = builtin_presets()
        .into_iter()
        .filter(|p| q.is_empty() || p.id.contains(&q) || p.name.to_ascii_lowercase().contains(&q))
        .map(|p| json!({ "id": p.id, "name": p.name }))
        .collect();
    let camera_profiles: Vec<Value> = builtin_camera_profiles()
        .into_iter()
        .filter(|p| {
            q.is_empty()
                || p.id.contains(&q)
                || p.name.to_ascii_lowercase().contains(&q)
                || p.description.to_ascii_lowercase().contains(&q)
                || p.category.as_str().contains(&q)
        })
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "category": p.category,
                "description": p.description,
            })
        })
        .collect();

    let all_cmds = [
        (
            "rd inspect <photo>",
            "Read EXIF, dimensions, and sidecar state",
        ),
        (
            "rd analyze <paths...>",
            "Perceptual tone, color cast, skin, palette, sharpness, auto tone/WB",
        ),
        (
            "rd edit <paths...>",
            "Modify `.rune-edit.json` via --set, --preset, --profile, --profile-strength, --wb-preset, --film-curve, --processing-mode, --pick-film-base, --vignette-method, --pick-skin, --pick-color-slice, --auto, --patch, --reset",
        ),
        (
            "rd cull <paths...>",
            "Batch star rating, Pick/Reject culling, and 7 Color Tags (--color-tag)",
        ),
        (
            "rd variant list|new|clone|select|delete <photo>",
            "Manage Virtual Variants on a photo sidecar",
        ),
        (
            "rd style list|save|apply|delete",
            "Save, apply, or additively stack reusable User Styles",
        ),
        (
            "rd copy-look <src> <dst...>",
            "Selective Adjustments Clipboard (--groups, --autoselect-adjusted)",
        ),
        (
            "rd layer list|add|update|mask-add|mask-clear|remove <photo>",
            "Adjustment Layers, Combine Masks (Linear/Radial/Luma), AI Subject/Background, and interactive AI Select (--kind ai-select --point --box --engine --invert)",
        ),
        (
            "rd model list|status|setup|use|remove",
            "Manage AI segmentation models & engines (Apple Vision ANE, SAM 2.1, SAM 3 ONNX; --accept-license, --from-dir)",
        ),
        (
            "rd heal list|add|remove|clear <photo>",
            "Spot Heal and Clone retouching stamps",
        ),
        (
            "rd session scan <dir>",
            "Scan and filter a photo directory (--color-tag, --search, --picks, --min-rating)",
        ),
        (
            "rd session init <dir>",
            "Create a Capture One 4-folder session (Capture/Selects/Output/Trash)",
        ),
        (
            "rd session watch <dir>",
            "Hot-folder auto-ingest watcher (--style, --once, --interval-ms)",
        ),
        (
            "rd preset list|show|apply|save",
            "Manage built-in and custom styles",
        ),
        (
            "rd profile list|import|delete",
            "List Camera Calibration/Film Simulation & user profiles (--camera, --make), import .dcp/.icc/.icm/.cube, or delete user profiles",
        ),
        (
            "rd preview <photo> -o <out.jpg>",
            "Render fast 1280px preview for visual inspection",
        ),
        (
            "rd diff <photo> [baseline]",
            "CIEDE2000 perceptual diff + --assert guardrails",
        ),
        (
            "rd export <paths...> -o <dest>",
            "Full-resolution or resized export",
        ),
        (
            "rd batch run <job.json|->",
            "Conditional batch rules and multi-target exports",
        ),
        (
            "rd open [path]",
            "Launch Rune Darkroom GUI on a photo or folder",
        ),
    ];
    let commands: Vec<Value> = all_cmds
        .iter()
        .filter(|(c, d)| {
            q.is_empty()
                || c.to_ascii_lowercase().contains(&q)
                || d.to_ascii_lowercase().contains(&q)
        })
        .map(|(c, d)| json!({ "command": c, "summary": d }))
        .collect();

    let mut lines = Vec::new();
    if !commands.is_empty() {
        lines.push("Commands:".to_owned());
        for c in &commands {
            lines.push(format!(
                "  {} — {}",
                c["command"].as_str().unwrap_or(""),
                c["summary"].as_str().unwrap_or("")
            ));
        }
    }
    if !params.is_empty() {
        lines.push("Parameters:".to_owned());
        for p in &params {
            let bounds = match (p.min, p.max) {
                (Some(lo), Some(hi)) => format!(" [{lo}..={hi}]"),
                _ => String::new(),
            };
            lines.push(format!("  {}{} — {}", p.id, bounds, p.description));
        }
    }
    if !presets.is_empty() {
        lines.push("Presets:".to_owned());
        for p in &presets {
            lines.push(format!(
                "  {} ({})",
                p["id"].as_str().unwrap_or(""),
                p["name"].as_str().unwrap_or("")
            ));
        }
    }
    if !camera_profiles.is_empty() {
        lines.push("Camera Profiles:".to_owned());
        for p in &camera_profiles {
            lines.push(format!(
                "  {} ({}) — {}",
                p["id"].as_str().unwrap_or(""),
                p["name"].as_str().unwrap_or(""),
                p["description"].as_str().unwrap_or("")
            ));
        }
    }

    let data = json!({
        "query": query,
        "commands": commands,
        "parameters": params,
        "presets": presets,
        "camera_profiles": camera_profiles,
    });
    emit_ok(opts, "cli.search", data, &lines.join("\n"));
    Ok(exit_code::OK)
}

fn inspect_photo_payload(
    args: &InspectArgs,
    opts: &RuntimeOpts,
) -> Result<(Value, String), CliError> {
    ensure_exists(&args.path)?;
    let sidecar_path = DevelopRecipe::sidecar_path_for(&args.path);
    let loaded_meta = PhotoMeta::load_sidecar(&args.path)?;
    let has_sidecar = loaded_meta.is_some();
    let meta = loaded_meta.unwrap_or_default();
    let exif = extract_exif_from_path(&args.path);
    let file_size = std::fs::metadata(&args.path).map(|m| m.len()).unwrap_or(0);

    let include_full = args.include.iter().any(|s| s == "full_recipe") || opts.verbose;
    let include_history = args.include.iter().any(|s| s == "history") || opts.verbose;
    let include_hist = args.include.iter().any(|s| s == "histogram");

    let wb_kelvin = temperature_to_kelvin(meta.recipe.temperature);
    let variants_summary = build_variants_summary(&meta);
    let layers_summary = build_layers_summary(&meta.recipe.layers);

    let mut data = json!({
        "path": args.path,
        "file_size_bytes": file_size,
        "is_raw": is_raw_extension(&args.path),
        "has_sidecar": has_sidecar,
        "sidecar_path": sidecar_path,
        "meta": {
            "meta_version": meta.meta_version,
            "revision": meta.revision,
            "rating": meta.rating,
            "flag": meta.flag,
            "color_tag": meta.color_tag,
            "active_variant_index": meta.active_variant_index,
            "variant_count": meta.variant_count(),
            "history_count": meta.history.len(),
        },
        "color_tag": meta.color_tag,
        "active_variant_index": meta.active_variant_index,
        "variant_count": meta.variant_count(),
        "variants": variants_summary,
        "layer_count": meta.recipe.layers.len(),
        "layers": layers_summary,
        "heal_spot_count": meta.recipe.heal_spots.len(),
        "exif": exif,
        "wb_kelvin": wb_kelvin,
        "wb_preset": meta.recipe.wb_preset,
        "camera_profile": meta.recipe.camera_profile,
        "profile_strength": meta.recipe.profile_strength,
        "film_curve": meta.recipe.film_curve,
        "processing_mode": meta.recipe.processing_mode,
        "film_negative": meta.recipe.film_negative,
        "recipe_overrides": recipe_non_default_map(&meta.recipe),
    });

    if let Some(obj) = data.as_object_mut() {
        if include_full {
            obj.insert("recipe".to_owned(), json!(meta.recipe));
        }
        if include_history {
            obj.insert("history".to_owned(), json!(meta.history));
        }
        if include_hist {
            let proxy = load_file_proxy(&args.path, Some(320))?;
            let hist = compute_histogram(&proxy.histogram_proxy, &meta.recipe);
            obj.insert("dimensions".to_owned(), json!([proxy.width, proxy.height]));
            obj.insert(
                "histogram".to_owned(),
                json!({
                    "shadow_clip_pct": hist.shadow_clip_pct,
                    "highlight_clip_pct": hist.highlight_clip_pct,
                    "luma": hist.luma.as_slice(),
                }),
            );
        }
    }

    let exif_line = exif
        .as_ref()
        .map(|e| e.exposure_summary())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "no EXIF".to_owned());
    let neg_note = if meta.recipe.processing_mode == ProcessingMode::FilmNegative {
        let f = &meta.recipe.film_negative;
        format!(
            " | Film Negative: base=({:.2},{:.2},{:.2}) density={:+.2} contrast={:+.2}",
            f.base_r, f.base_g, f.base_b, f.density, f.contrast
        )
    } else {
        String::new()
    };
    let human = format!(
        "{}\n  Size: {} KB | Sidecar: {} (rev {}) | Rating: {}★ | Flag: {:?} | Tag: {} | Variant: V{}/{}\n  Layers: {} | Heal Spots: {} | Mode: {}{} | Profile: {} ({:.0}%) | WB: {} K ({}) | Curve: {} | EXIF: {}\n  Overrides: {}",
        args.path.display(),
        file_size / 1024,
        if has_sidecar { "yes" } else { "no" },
        meta.revision,
        meta.rating,
        meta.flag,
        meta.color_tag.label(),
        meta.active_variant_index + 1,
        meta.variant_count(),
        meta.recipe.layers.len(),
        meta.recipe.heal_spots.len(),
        meta.recipe.processing_mode.label(),
        neg_note,
        meta.recipe.camera_profile,
        meta.recipe.profile_strength * 100.0,
        wb_kelvin,
        meta.recipe.wb_preset.label(),
        meta.recipe.film_curve.label(),
        exif_line,
        recipe_non_default_map(&meta.recipe).len()
    );
    Ok((data, human))
}

fn cmd_inspect(args: InspectArgs, opts: &RuntimeOpts) -> Result<u8, CliError> {
    let (data, human) = inspect_photo_payload(&args, opts)?;
    emit_ok(opts, "inspect", data, &human);
    Ok(exit_code::OK)
}

fn cmd_analyze(args: AnalyzeArgs, opts: &RuntimeOpts) -> Result<u8, CliError> {
    let files = expand_input_paths(&args.paths, args.recursive, &[])?;
    if files.is_empty() {
        return Err(CliError::not_found(
            "No supported photos found in given paths.",
        ));
    }

    let sample_xy = if let Some(s) = &args.sample {
        Some(parse_xy_pair(s)?)
    } else {
        None
    };

    let multi = files.len() > 1 || matches!(opts.format, ResolvedFormat::Ndjson);
    let mut results = Vec::new();
    let mut failed = 0usize;

    for path in &files {
        match analyze_single_photo(path, &args, sample_xy) {
            Ok(item) => {
                if matches!(opts.format, ResolvedFormat::Ndjson) {
                    emit_ndjson_line(&item);
                } else {
                    results.push(item);
                }
            }
            Err(err) => {
                failed += 1;
                if matches!(opts.format, ResolvedFormat::Ndjson) {
                    emit_ndjson_line(&json!({
                        "type": "error",
                        "path": path,
                        "code": err.code,
                        "message": err.message,
                    }));
                } else if !multi {
                    return Err(err);
                } else {
                    results.push(json!({
                        "path": path,
                        "ok": false,
                        "error": err.message,
                    }));
                }
            }
        }
    }

    if matches!(opts.format, ResolvedFormat::Ndjson) {
        return Ok(if failed == 0 {
            exit_code::OK
        } else {
            exit_code::PARTIAL_OR_ASSERT
        });
    }

    if !multi {
        let item = results.into_iter().next().unwrap_or(json!({}));
        let tone = &item["analysis"]["tone"];
        let focus_score = item["analysis"]["focus"]["sharpness_score"]
            .as_f64()
            .map(|v| format!("{v:.1}"))
            .unwrap_or_else(|| "n/a".to_owned());
        let human = format!(
            "{}\n  Tone: key={} DR={:.1} EV | Clip S={:.2}% H={:.2}% | HDR headroom={:.1}%\n  Cast: {} (a*={:.1}, b*={:.1}) | Sharpness: {}\n  Auto suggest: EV={:+.2} Hi={:+.2} Sh={:+.2} Temp={:+.2} Tint={:+.2}",
            files[0].display(),
            tone["key"].as_str().unwrap_or("?"),
            tone["dynamic_range_stops_p01_p99"].as_f64().unwrap_or(0.0),
            tone["clip_pct"]["shadow_all"].as_f64().unwrap_or(0.0),
            tone["clip_pct"]["highlight_any"].as_f64().unwrap_or(0.0),
            tone["hdr_headroom_pct"].as_f64().unwrap_or(0.0),
            item["analysis"]["color"]["cast_lab_neutral"]["note"]
                .as_str()
                .unwrap_or("?"),
            item["analysis"]["color"]["cast_lab_neutral"]["a_star"]
                .as_f64()
                .unwrap_or(0.0),
            item["analysis"]["color"]["cast_lab_neutral"]["b_star"]
                .as_f64()
                .unwrap_or(0.0),
            focus_score,
            item["analysis"]["suggested_auto"]["exposure"]
                .as_f64()
                .unwrap_or(0.0),
            item["analysis"]["suggested_auto"]["highlights"]
                .as_f64()
                .unwrap_or(0.0),
            item["analysis"]["suggested_auto"]["shadows"]
                .as_f64()
                .unwrap_or(0.0),
            item["analysis"]["suggested_auto"]["temperature"]
                .as_f64()
                .unwrap_or(0.0),
            item["analysis"]["suggested_auto"]["tint"]
                .as_f64()
                .unwrap_or(0.0),
        );
        emit_ok(opts, "analyze", item, &human);
    } else {
        let human = format!("Analyzed {} photo(s) ({failed} failed).", files.len());
        emit_ok(
            opts,
            "analyze",
            json!({ "total": files.len(), "failed": failed, "items": results }),
            &human,
        );
    }

    Ok(if failed == 0 {
        exit_code::OK
    } else {
        exit_code::PARTIAL_OR_ASSERT
    })
}

fn analyze_single_photo(
    path: &Path,
    args: &AnalyzeArgs,
    sample_xy: Option<[f32; 2]>,
) -> Result<Value, CliError> {
    let proxy = load_file_proxy(path, Some(args.proxy_edge.max(64)))?;
    let meta = PhotoMeta::load_sidecar(path)?.unwrap_or_default();
    let detail = if !args.fast && args.detail_edge > 0 {
        extract_detail_luma_proxy(path, args.detail_edge).ok()
    } else {
        None
    };
    let detail_ref = detail.as_ref().map(|(buf, w, h)| (buf.as_slice(), *w, *h));
    let analysis = analyze_linear_proxy(&proxy.histogram_proxy, &meta.recipe, detail_ref);

    let mut obj = json!({
        "path": path,
        "proxy_dimensions": [proxy.width, proxy.height],
        "meta": {
            "rating": meta.rating,
            "flag": meta.flag,
            "revision": meta.revision,
        },
        "recipe_overrides": recipe_non_default_map(&meta.recipe),
        "analysis": analysis,
    });

    if args.include.iter().any(|s| s == "exif") || proxy.exif.is_some() {
        obj["exif"] = json!(proxy.exif);
    }

    if let Some([nx, ny]) = sample_xy {
        let raw_rgb = sample_patch_3x3(&proxy, nx, ny);
        let compiled = meta.recipe.compile();
        let graded_lin = filmic_tonemap_with_curve(
            compiled.evaluate_pixel_scene_linear(raw_rgb),
            meta.recipe.film_curve,
        );
        let graded_srgb = [
            linear_to_srgb(graded_lin[0]),
            linear_to_srgb(graded_lin[1]),
            linear_to_srgb(graded_lin[2]),
        ];
        let (hsl, lab) = rgb_to_hsl_and_lab(graded_srgb);
        let (wb_temp, wb_tint) = pick_white_balance(raw_rgb);
        obj["sample"] = json!({
            "coord": [nx, ny],
            "raw_linear_rgb": raw_rgb,
            "graded_srgb": graded_srgb,
            "hsl": hsl,
            "lab": lab,
            "wb_neutralizer": {
                "temperature": wb_temp,
                "tint": wb_tint,
                "wb_kelvin": temperature_to_kelvin(wb_temp),
            },
        });
    }

    Ok(obj)
}

fn cmd_edit(args: EditArgs, opts: &RuntimeOpts) -> Result<u8, CliError> {
    let mut files = expand_input_paths(&args.paths, args.recursive, &[])?;
    if files.is_empty() {
        return Err(CliError::not_found("No supported photos found for edit."));
    }

    // Apply optional cull filters before editing.
    if args.picks || args.min_rating.is_some() {
        files.retain(|p| {
            let m = PhotoMeta::load_sidecar(p)
                .ok()
                .flatten()
                .unwrap_or_default();
            let ok_pick = !args.picks || m.flag == CullFlag::Pick;
            let ok_rate = args.min_rating.is_none_or(|min| m.rating >= min);
            ok_pick && ok_rate
        });
    }

    // Pre-parse JSON patch if given.
    let parsed_patch: Option<RecipePatch> = match &args.patch {
        Some(raw) => {
            let text = read_arg_or_stdin(raw)?;
            let patch: RecipePatch = serde_json::from_str(&text)
                .map_err(|e| CliError::validation(format!("Invalid RecipePatch JSON: {e}")))?;
            Some(patch)
        }
        None => None,
    };

    let preset_recipe: Option<DevelopRecipe> = match &args.preset {
        Some(p) => Some(load_preset_or_file(p)?),
        None => None,
    };

    let copy_recipe: Option<DevelopRecipe> = match &args.copy_from {
        Some(p) => {
            ensure_exists(p)?;
            Some(PhotoMeta::load_sidecar(p)?.unwrap_or_default().recipe)
        }
        None => None,
    };

    let wb_pick_xy = match &args.wb_pick {
        Some(s) => Some(parse_xy_pair(s)?),
        None => None,
    };
    let film_base_pick_xy = match &args.pick_film_base {
        Some(s) => Some(parse_xy_pair(s)?),
        None => None,
    };
    let skin_pick_xy = match &args.pick_skin {
        Some(s) => Some(parse_xy_pair(s)?),
        None => None,
    };
    let mut color_slice_picks = Vec::with_capacity(args.pick_color_slices.len());
    for raw in &args.pick_color_slices {
        color_slice_picks.push(parse_color_slice_pick(raw)?);
    }

    // Merge typed shorthand flags into `--set` expressions (`--wb-preset`, `--film-curve`,
    // `--vignette-method`, `--pick-skin`, and `--pick-color-slice` are applied in
    // `edit_single_photo` before `--patch` to match MCP `rune_edit_photo` 1:1).
    let mut all_sets = args.sets.clone();
    for (key, opt) in [
        ("exposure", &args.exposure),
        ("contrast", &args.contrast),
        ("highlights", &args.highlights),
        ("shadows", &args.shadows),
        ("temperature", &args.temp),
        ("tint", &args.tint),
        ("vibrance", &args.vibrance),
        ("saturation", &args.saturation),
        ("clarity", &args.clarity),
        ("structure", &args.structure),
        ("dehaze", &args.dehaze),
        ("sharpen", &args.sharpen),
        ("noise_reduction", &args.denoise),
        ("geometry.aspect_preset", &args.aspect),
    ] {
        if let Some(val) = opt {
            all_sets.push(format!("{key}={val}"));
        }
    }

    let mut edited = Vec::new();
    let mut failed = 0usize;

    for path in &files {
        match edit_single_photo(
            path,
            &args,
            &all_sets,
            preset_recipe.as_ref(),
            copy_recipe.as_ref(),
            parsed_patch.as_ref(),
            wb_pick_xy,
            film_base_pick_xy,
            skin_pick_xy,
            &color_slice_picks,
            opts,
        ) {
            Ok(item) => {
                if matches!(opts.format, ResolvedFormat::Ndjson) {
                    emit_ndjson_line(&item);
                } else {
                    edited.push(item);
                }
            }
            Err(err) => {
                failed += 1;
                if files.len() == 1 {
                    return Err(err);
                }
                let err_val = json!({
                    "type": "error",
                    "path": path,
                    "code": err.code,
                    "message": err.message,
                });
                if matches!(opts.format, ResolvedFormat::Ndjson) {
                    emit_ndjson_line(&err_val);
                } else {
                    edited.push(err_val);
                }
            }
        }
    }

    if matches!(opts.format, ResolvedFormat::Ndjson) {
        return Ok(if failed == 0 {
            exit_code::OK
        } else {
            exit_code::PARTIAL_OR_ASSERT
        });
    }

    if files.len() == 1 {
        let item = edited.into_iter().next().unwrap_or(json!({}));
        let n_changes = item["changed"].as_array().map_or(0, Vec::len);
        let human = format!(
            "{} {} ({n_changes} field change(s))",
            if opts.dry_run {
                "[dry-run] Would update"
            } else {
                "Updated"
            },
            files[0].display(),
        );
        emit_ok(opts, "edit", item, &human);
    } else {
        let human = format!(
            "{} {} photo(s) ({failed} failed).",
            if opts.dry_run {
                "[dry-run] Would update"
            } else {
                "Updated"
            },
            files.len() - failed,
        );
        emit_ok(
            opts,
            "edit",
            json!({ "total": files.len(), "succeeded": files.len() - failed, "failed": failed, "items": edited }),
            &human,
        );
    }

    Ok(if failed == 0 {
        exit_code::OK
    } else {
        exit_code::PARTIAL_OR_ASSERT
    })
}

#[allow(clippy::too_many_arguments)]
fn edit_single_photo(
    path: &Path,
    args: &EditArgs,
    sets: &[String],
    preset_recipe: Option<&DevelopRecipe>,
    copy_recipe: Option<&DevelopRecipe>,
    patch: Option<&RecipePatch>,
    wb_pick_xy: Option<[f32; 2]>,
    film_base_pick_xy: Option<[f32; 2]>,
    skin_pick_xy: Option<[f32; 2]>,
    color_slice_picks: &[(usize, [f32; 2])],
    opts: &RuntimeOpts,
) -> Result<Value, CliError> {
    let mut meta = PhotoMeta::load_sidecar(path)?.unwrap_or_default();
    let base_revision = meta.revision;
    let before = meta.recipe.clone();

    if let Some(group) = &args.reset {
        reset_recipe_group(&mut meta.recipe, group)?;
    }

    if let Some(src) = copy_recipe {
        copy_recipe_groups(&mut meta.recipe, src, &args.only)?;
    }

    if let Some(preset) = preset_recipe {
        let mut target = meta.recipe.clone();
        if args.only.is_empty() {
            let keep_geo = target.geometry;
            target = preset.clone();
            target.geometry = keep_geo;
        } else {
            copy_recipe_groups(&mut target, preset, &args.only)?;
        }
        meta.recipe = blend_recipes(&meta.recipe, &target, args.mix);
        if (args.mix - 1.0).abs() <= 1e-4 && args.only.is_empty() {
            meta.recipe.preset_name = preset.preset_name.clone();
        }
    }

    let need_proxy = !args.auto.is_empty()
        || wb_pick_xy.is_some()
        || film_base_pick_xy.is_some()
        || skin_pick_xy.is_some()
        || !color_slice_picks.is_empty();
    let need_dims = args.aspect.is_some()
        || patch.is_some_and(|p| p.geometry.is_some())
        || sets_touch_geometry(sets);
    let proxy = if need_proxy {
        Some(load_file_proxy(path, Some(320))?)
    } else {
        None
    };
    let src_dims = if need_dims {
        Some(probe_image_dimensions(path)?)
    } else {
        proxy.as_ref().map(|p| [p.width, p.height])
    };

    if let Some(p) = &proxy {
        for mode in &args.auto {
            match mode.trim().to_ascii_lowercase().as_str() {
                "wb" => {
                    let (t, ti) = auto_white_balance(&p.histogram_proxy);
                    meta.recipe.temperature = t;
                    meta.recipe.tint = ti;
                    meta.recipe.wb_preset = WbPreset::Custom;
                }
                "tone" | "exposure" => {
                    let (e, c, hi, sh) = auto_exposure_and_tone(&p.histogram_proxy);
                    meta.recipe.exposure = e;
                    meta.recipe.contrast = c;
                    meta.recipe.highlights = hi;
                    meta.recipe.shadows = sh;
                }
                "levels" => {
                    meta.recipe.levels = auto_levels(&p.histogram_proxy);
                }
                "all" => {
                    let (t, ti) = auto_white_balance(&p.histogram_proxy);
                    let (e, c, hi, sh) = auto_exposure_and_tone(&p.histogram_proxy);
                    meta.recipe.temperature = t;
                    meta.recipe.tint = ti;
                    meta.recipe.wb_preset = WbPreset::Custom;
                    meta.recipe.exposure = e;
                    meta.recipe.contrast = c;
                    meta.recipe.highlights = hi;
                    meta.recipe.shadows = sh;
                    meta.recipe.levels = auto_levels(&p.histogram_proxy);
                }
                other => {
                    return Err(CliError::validation(format!(
                        "Unknown --auto mode '{other}'. Valid: wb, tone, levels, all"
                    )));
                }
            }
            meta.recipe.preset_name = None;
        }

        if let Some([nx, ny]) = wb_pick_xy {
            let patch_rgb = sample_patch_3x3(p, nx, ny);
            let (t, ti) = pick_white_balance(patch_rgb);
            meta.recipe.temperature = t;
            meta.recipe.tint = ti;
            meta.recipe.wb_preset = WbPreset::Custom;
            meta.recipe.preset_name = None;
        }
    }

    if let Some(pm_str) = &args.processing_mode {
        set_param_by_str(&mut meta.recipe, "processing_mode", pm_str, false, src_dims)?;
    }

    if let Some(p) = &proxy
        && let Some([nx, ny]) = film_base_pick_xy
    {
        let raw_rgb = sample_raw_linear_rgb_at_uv(p, nx, ny, &meta.recipe);
        meta.recipe.convert_film_negative_from_base(raw_rgb);
        meta.recipe.preset_name = None;
    }

    if let Some(wb_str) = &args.wb_preset {
        set_param_by_str(&mut meta.recipe, "wb_preset", wb_str, false, src_dims)?;
    }

    if let Some(profile_id) = &args.profile {
        set_param_by_str(
            &mut meta.recipe,
            "camera_profile",
            profile_id,
            false,
            src_dims,
        )?;
    }

    if let Some(strength) = args.profile_strength {
        if !strength.is_finite() || !(0.0..=1.0).contains(&strength) {
            return Err(CliError::validation(format!(
                "--profile-strength must be within 0.0..=1.0 (got {strength})"
            )));
        }
        set_param_by_str(
            &mut meta.recipe,
            "profile_strength",
            &strength.to_string(),
            false,
            src_dims,
        )?;
    }

    if let Some(fc_str) = &args.film_curve {
        set_param_by_str(&mut meta.recipe, "film_curve", fc_str, false, src_dims)?;
    }

    if let Some(vm_str) = &args.vignette_method {
        set_param_by_str(&mut meta.recipe, "vignette_method", vm_str, false, src_dims)?;
    }

    if let Some(p) = &proxy {
        if let Some([nx, ny]) = skin_pick_xy {
            let rgb = sample_linear_rgb_at_uv(p, nx, ny, &meta.recipe);
            meta.recipe.pick_skin_tone_from_rgb(rgb);
            meta.recipe.preset_name = None;
        }
        for &(slice_idx, [nx, ny]) in color_slice_picks {
            let rgb = sample_linear_rgb_at_uv(p, nx, ny, &meta.recipe);
            meta.recipe.pick_color_slice_from_rgb(slice_idx, rgb);
            meta.recipe.preset_name = None;
        }
    }

    if let Some(rp) = patch {
        rp.apply_to(&mut meta.recipe, src_dims)?;
    }

    for expr in sets {
        let (key, val, delta) = parse_set_expr(expr)?;
        set_param_by_str(&mut meta.recipe, key, &val, delta, src_dims)?;
    }

    meta.recipe = meta.recipe.sanitized();
    let diffs = recipe_field_diffs(&before, &meta.recipe);
    let sidecar_path = DevelopRecipe::sidecar_path_for(path);

    if !opts.dry_run && !diffs.is_empty() {
        let summary = format!("rd edit ({}): {} field(s)", args.agent, diffs.len());
        meta.push_history(
            EditActor::Agent {
                name: args.agent.clone(),
                job_id: None,
            },
            summary,
            diffs.clone(),
            Some(before),
        );
        let expected = if args.force || base_revision == 0 {
            None
        } else {
            Some(base_revision)
        };
        meta.save_sidecar_checked_in_place(path, expected)?;
    }

    Ok(json!({
        "path": path,
        "sidecar": sidecar_path,
        "revision": meta.revision,
        "wb_kelvin": temperature_to_kelvin(meta.recipe.temperature),
        "camera_profile": meta.recipe.camera_profile,
        "profile_strength": meta.recipe.profile_strength,
        "processing_mode": meta.recipe.processing_mode,
        "changed": diffs,
        "recipe_overrides": recipe_non_default_map(&meta.recipe),
    }))
}

fn cmd_cull(args: CullArgs, opts: &RuntimeOpts) -> Result<u8, CliError> {
    if args.rate.is_none() && !args.pick && !args.reject && !args.unflag && args.color_tag.is_none()
    {
        return Err(CliError::validation(
            "Specify at least one of --rate, --pick, --reject, --unflag, or --color-tag.",
        ));
    }
    if let Some(r) = args.rate
        && r > 5
    {
        return Err(CliError::validation(format!(
            "--rate must be within 0..=5 (got {r})"
        )));
    }
    let parsed_color_tag = match &args.color_tag {
        Some(raw) => Some(parse_color_tag(raw)?),
        None => None,
    };

    let mut inputs = args.paths.clone();
    if args.from_stdin {
        for line in io::stdin().lock().lines() {
            let line = line.map_err(|e| CliError::io(e.to_string()))?;
            let t = line.trim();
            if !t.is_empty() {
                inputs.push(PathBuf::from(t));
            }
        }
    }
    let files = expand_input_paths(&inputs, false, &[])?;
    if files.is_empty() {
        return Err(CliError::not_found(
            "No photo targets supplied to `rd cull`.",
        ));
    }

    let mut updated = Vec::new();
    for path in &files {
        let mut meta = PhotoMeta::load_sidecar(path)?.unwrap_or_default();
        if let Some(r) = args.rate {
            meta.rating = r;
        }
        if args.pick {
            meta.flag = CullFlag::Pick;
        } else if args.reject {
            meta.flag = CullFlag::Reject;
        } else if args.unflag {
            meta.flag = CullFlag::None;
        }
        if let Some(tag) = parsed_color_tag {
            meta.color_tag = tag;
        }
        if !opts.dry_run {
            meta.save_sidecar_in_place(path)?;
        }
        updated.push(json!({
            "path": path,
            "rating": meta.rating,
            "flag": meta.flag,
            "color_tag": meta.color_tag,
        }));
    }

    let human = format!(
        "{} {} photo(s).",
        if opts.dry_run {
            "[dry-run] Would cull"
        } else {
            "Updated cull metadata on"
        },
        updated.len()
    );
    emit_ok(
        opts,
        "cull",
        json!({ "count": updated.len(), "items": updated }),
        &human,
    );
    Ok(exit_code::OK)
}

fn cmd_variant(sub: VariantCmd, opts: &RuntimeOpts) -> Result<u8, CliError> {
    match sub {
        VariantCmd::List { photo } => {
            ensure_exists(&photo)?;
            let mut meta = PhotoMeta::load_sidecar(&photo)?.unwrap_or_default();
            meta.ensure_variants();
            let variants = build_variants_summary(&meta);
            let data = json!({
                "path": photo,
                "active_variant_index": meta.active_variant_index,
                "variant_count": meta.variant_count(),
                "variants": variants,
            });
            let human = format!(
                "{}: {} variant(s) (active V{})",
                photo.display(),
                meta.variant_count(),
                meta.active_variant_index + 1
            );
            emit_ok(opts, "variant.list", data, &human);
            Ok(exit_code::OK)
        }
        VariantCmd::New { photo, label } => {
            ensure_exists(&photo)?;
            let mut meta = PhotoMeta::load_sidecar(&photo)?.unwrap_or_default();
            let idx = meta.new_variant(label);
            if !opts.dry_run {
                meta.save_sidecar_in_place(&photo)?;
            }
            let data = json!({
                "path": photo,
                "created_index": idx,
                "active_variant_index": meta.active_variant_index,
                "variant_count": meta.variant_count(),
                "variants": build_variants_summary(&meta),
            });
            let human = format!(
                "Created variant V{} on {} ({} total)",
                idx + 1,
                photo.display(),
                meta.variant_count()
            );
            emit_ok(opts, "variant.new", data, &human);
            Ok(exit_code::OK)
        }
        VariantCmd::Clone { photo, label } => {
            ensure_exists(&photo)?;
            let mut meta = PhotoMeta::load_sidecar(&photo)?.unwrap_or_default();
            let idx = meta.clone_variant(label);
            if !opts.dry_run {
                meta.save_sidecar_in_place(&photo)?;
            }
            let data = json!({
                "path": photo,
                "cloned_index": idx,
                "active_variant_index": meta.active_variant_index,
                "variant_count": meta.variant_count(),
                "variants": build_variants_summary(&meta),
            });
            let human = format!(
                "Cloned variant into V{} on {} ({} total)",
                idx + 1,
                photo.display(),
                meta.variant_count()
            );
            emit_ok(opts, "variant.clone", data, &human);
            Ok(exit_code::OK)
        }
        VariantCmd::Select { photo, index } => {
            ensure_exists(&photo)?;
            let mut meta = PhotoMeta::load_sidecar(&photo)?.unwrap_or_default();
            if !meta.select_variant(index) {
                return Err(CliError::validation(format!(
                    "Variant index {index} out of range (photo has {} variant(s))",
                    meta.variant_count()
                )));
            }
            if !opts.dry_run {
                meta.save_sidecar_in_place(&photo)?;
            }
            let data = json!({
                "path": photo,
                "active_variant_index": meta.active_variant_index,
                "variant_count": meta.variant_count(),
                "variants": build_variants_summary(&meta),
            });
            let human = format!("Selected variant V{} on {}", index + 1, photo.display());
            emit_ok(opts, "variant.select", data, &human);
            Ok(exit_code::OK)
        }
        VariantCmd::Delete { photo, index } => {
            ensure_exists(&photo)?;
            let mut meta = PhotoMeta::load_sidecar(&photo)?.unwrap_or_default();
            if !meta.delete_variant(index) {
                return Err(CliError::validation(format!(
                    "Cannot delete variant index {index} (photo has {} variant(s); at least 1 variant must remain)",
                    meta.variant_count()
                )));
            }
            if !opts.dry_run {
                meta.save_sidecar_in_place(&photo)?;
            }
            let data = json!({
                "path": photo,
                "deleted_index": index,
                "active_variant_index": meta.active_variant_index,
                "variant_count": meta.variant_count(),
                "variants": build_variants_summary(&meta),
            });
            let human = format!(
                "Deleted variant {} on {} ({} remaining, active V{})",
                index + 1,
                photo.display(),
                meta.variant_count(),
                meta.active_variant_index + 1
            );
            emit_ok(opts, "variant.delete", data, &human);
            Ok(exit_code::OK)
        }
    }
}

fn cmd_style(sub: StyleCmd, opts: &RuntimeOpts) -> Result<u8, CliError> {
    match sub {
        StyleCmd::List { styles_dir } => {
            let dir = styles_dir.unwrap_or_else(default_styles_dir);
            let styles = list_user_styles_in_dir(&dir)?;
            let items: Vec<Value> = styles
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
            let human = if items.is_empty() {
                format!("No saved User Styles in {}", dir.display())
            } else {
                items
                    .iter()
                    .map(|s| {
                        format!(
                            "  {:24} {}",
                            s["slug"].as_str().unwrap_or(""),
                            s["name"].as_str().unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            emit_ok(
                opts,
                "style.list",
                json!({ "styles_dir": dir, "count": items.len(), "styles": items }),
                &human,
            );
            Ok(exit_code::OK)
        }
        StyleCmd::Save {
            photo,
            name,
            description,
            groups,
            autoselect_adjusted,
            styles_dir,
        } => {
            ensure_exists(&photo)?;
            let recipe = PhotoMeta::load_sidecar(&photo)?.unwrap_or_default().recipe;
            let effective_groups: Vec<String> = if autoselect_adjusted && groups.is_empty() {
                modified_recipe_groups(&recipe)
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            } else {
                groups
            };
            let style = UserStyle::from_recipe(&name, &description, &recipe, &effective_groups)?;
            let dir = styles_dir.unwrap_or_else(default_styles_dir);
            let saved_path = if opts.dry_run {
                dir.join(format!("{}.json", style.slug()?))
            } else {
                save_user_style_in_dir(&dir, &style)?
            };
            let human = format!(
                "{} User Style '{}' ({} group(s)) -> {}",
                if opts.dry_run {
                    "[dry-run] Would save"
                } else {
                    "Saved"
                },
                style.name,
                style.groups.len(),
                saved_path.display()
            );
            emit_ok(
                opts,
                "style.save",
                json!({
                    "name": style.name,
                    "slug": style.slug()?,
                    "groups": style.groups,
                    "path": saved_path,
                }),
                &human,
            );
            Ok(exit_code::OK)
        }
        StyleCmd::Apply {
            name,
            paths,
            stack,
            strength,
            styles_dir,
        } => {
            if !strength.is_finite() || !(0.0..=1.0).contains(&strength) {
                return Err(CliError::validation(format!(
                    "--strength must be within 0.0..=1.0 (got {strength})"
                )));
            }
            let dir = styles_dir.unwrap_or_else(default_styles_dir);
            let style = load_user_style_in_dir(&dir, &name)?;
            let files = expand_input_paths(&paths, false, &[])?;
            if files.is_empty() {
                return Err(CliError::not_found(
                    "No supported photos found for style apply.",
                ));
            }
            let mut items = Vec::with_capacity(files.len());
            for path in &files {
                let mut meta = PhotoMeta::load_sidecar(path)?.unwrap_or_default();
                let before = meta.recipe.clone();
                if stack {
                    stack_style_onto_recipe(&mut meta.recipe, &style.patch, strength)?;
                } else {
                    apply_style_to_recipe(&mut meta.recipe, &style, strength)?;
                }
                meta.recipe = meta.recipe.sanitized();
                let diffs = recipe_field_diffs(&before, &meta.recipe);
                if !opts.dry_run && !diffs.is_empty() {
                    let mode_label = if stack { "stack" } else { "apply" };
                    meta.push_history(
                        EditActor::Agent {
                            name: "style".to_owned(),
                            job_id: None,
                        },
                        format!(
                            "rd style {mode_label} '{}' ({:.0}%)",
                            style.name,
                            strength * 100.0
                        ),
                        diffs.clone(),
                        Some(before),
                    );
                    meta.save_sidecar_in_place(path)?;
                }
                items.push(json!({
                    "path": path,
                    "changed_count": diffs.len(),
                    "revision": meta.revision,
                }));
            }
            let human = format!(
                "{} style '{}' onto {} photo(s)",
                if stack { "Stacked" } else { "Applied" },
                style.name,
                items.len()
            );
            emit_ok(
                opts,
                "style.apply",
                json!({
                    "name": style.name,
                    "stack": stack,
                    "strength": strength,
                    "count": items.len(),
                    "items": items,
                }),
                &human,
            );
            Ok(exit_code::OK)
        }
        StyleCmd::Delete { name, styles_dir } => {
            let dir = styles_dir.unwrap_or_else(default_styles_dir);
            let slug = rune_editor_core::sanitize_style_slug(&name)?;
            let deleted = if opts.dry_run {
                dir.join(format!("{slug}.json")).exists()
            } else {
                delete_user_style_in_dir(&dir, &name)?
            };
            if !deleted {
                return Err(CliError::not_found(format!(
                    "User Style '{name}' not found in {}",
                    dir.display()
                )));
            }
            let human = format!("Deleted User Style '{slug}'");
            emit_ok(
                opts,
                "style.delete",
                json!({ "name": name, "slug": slug, "deleted": true }),
                &human,
            );
            Ok(exit_code::OK)
        }
    }
}

fn cmd_copy_look(args: CopyLookArgs, opts: &RuntimeOpts) -> Result<u8, CliError> {
    ensure_exists(&args.src)?;
    let src_recipe = PhotoMeta::load_sidecar(&args.src)?
        .unwrap_or_default()
        .recipe;
    let groups: Vec<String> = if args.autoselect_adjusted {
        let mut selected: Vec<String> = modified_recipe_groups(&src_recipe)
            .into_iter()
            .map(str::to_owned)
            .collect();
        for g in &args.groups {
            if !selected.iter().any(|s| s.eq_ignore_ascii_case(g)) {
                selected.push(g.clone());
            }
        }
        selected
    } else {
        args.groups.clone()
    };

    let targets = expand_input_paths(&args.dst, args.recursive, &[])?;
    if targets.is_empty() {
        return Err(CliError::not_found(
            "No supported destination photos found for `rd copy-look`.",
        ));
    }

    let mut items = Vec::with_capacity(targets.len());
    for dst in &targets {
        let mut meta = PhotoMeta::load_sidecar(dst)?.unwrap_or_default();
        let before = meta.recipe.clone();
        if groups.is_empty() && args.autoselect_adjusted {
            // Source had no modified groups -> no-op cleanly.
        } else {
            copy_recipe_groups(&mut meta.recipe, &src_recipe, &groups)?;
        }
        meta.recipe = meta.recipe.sanitized();
        let diffs = recipe_field_diffs(&before, &meta.recipe);
        if !opts.dry_run && !diffs.is_empty() {
            meta.push_history(
                EditActor::Agent {
                    name: "copy-look".to_owned(),
                    job_id: None,
                },
                format!("rd copy-look ({} group(s))", groups.len()),
                diffs.clone(),
                Some(before),
            );
            meta.save_sidecar_in_place(dst)?;
        }
        items.push(json!({
            "path": dst,
            "changed_count": diffs.len(),
            "revision": meta.revision,
        }));
    }

    let human = format!(
        "Copied {} group(s) from {} to {} photo(s)",
        if groups.is_empty() && !args.autoselect_adjusted {
            "all".to_owned()
        } else {
            groups.len().to_string()
        },
        args.src.display(),
        items.len()
    );
    emit_ok(
        opts,
        "copy-look",
        json!({
            "src": args.src,
            "groups": groups,
            "autoselect_adjusted": args.autoselect_adjusted,
            "count": items.len(),
            "items": items,
        }),
        &human,
    );
    Ok(exit_code::OK)
}

#[allow(clippy::too_many_arguments)]
fn cmd_session_scan(
    dir: &Path,
    recursive: bool,
    picks: bool,
    rejects: bool,
    min_rating: Option<u8>,
    color_tag: Option<String>,
    search: Option<String>,
    ext: Vec<String>,
    opts: &RuntimeOpts,
) -> Result<u8, CliError> {
    ensure_exists(dir)?;
    let filter_tag = match color_tag.as_deref() {
        Some(raw) => Some(parse_color_tag(raw)?),
        None => None,
    };
    let search_q = search
        .as_deref()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty());

    let files = scan_photo_folder_with_options(
        dir,
        &ScanOptions {
            recursive,
            extensions: ext,
        },
    )?;

    let mut total_picks = 0usize;
    let mut total_rejects = 0usize;
    let mut total_edited = 0usize;
    let mut items = Vec::new();

    for path in files {
        let loaded = PhotoMeta::load_sidecar(&path)?;
        let has_sidecar = loaded.is_some();
        let meta = loaded.unwrap_or_default();
        match meta.flag {
            CullFlag::Pick => total_picks += 1,
            CullFlag::Reject => total_rejects += 1,
            CullFlag::None => {}
        }
        if !meta.recipe.is_neutral() {
            total_edited += 1;
        }
        if picks && meta.flag != CullFlag::Pick {
            continue;
        }
        if rejects && meta.flag != CullFlag::Reject {
            continue;
        }
        if let Some(min) = min_rating
            && meta.rating < min
        {
            continue;
        }
        if let Some(tag) = filter_tag
            && meta.color_tag != tag
        {
            continue;
        }
        if let Some(q) = &search_q {
            let fname = path
                .file_name()
                .map(|s| s.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            let preset_match = meta
                .recipe
                .preset_name
                .as_deref()
                .is_some_and(|p| p.to_ascii_lowercase().contains(q));
            let tag_match = meta.color_tag.label().to_ascii_lowercase().contains(q)
                || meta.color_tag.cli_slug().contains(q);
            let exif_match = if !fname.contains(q) && !preset_match && !tag_match {
                extract_exif_from_path(&path).is_some_and(|ex| {
                    ex.camera_model
                        .as_deref()
                        .is_some_and(|c| c.to_ascii_lowercase().contains(q))
                        || ex
                            .lens_model
                            .as_deref()
                            .is_some_and(|l| l.to_ascii_lowercase().contains(q))
                })
            } else {
                false
            };
            if !fname.contains(q) && !preset_match && !tag_match && !exif_match {
                continue;
            }
        }

        let item = json!({
            "path": path,
            "is_raw": is_raw_extension(&path),
            "has_sidecar": has_sidecar,
            "rating": meta.rating,
            "flag": meta.flag,
            "color_tag": meta.color_tag,
            "active_variant_index": meta.active_variant_index,
            "variant_count": meta.variant_count(),
            "edited": !meta.recipe.is_neutral(),
            "preset_name": meta.recipe.preset_name,
        });
        if matches!(opts.format, ResolvedFormat::Ndjson) {
            emit_ndjson_line(&item);
        } else {
            items.push(item);
        }
    }

    if matches!(opts.format, ResolvedFormat::Ndjson) {
        return Ok(exit_code::OK);
    }

    let human = format!(
        "Scanned {}: {} matched ({total_picks} picks, {total_rejects} rejects, {total_edited} edited)",
        dir.display(),
        items.len()
    );
    emit_ok(
        opts,
        "session.scan",
        json!({
            "dir": dir,
            "matched": items.len(),
            "picks": total_picks,
            "rejects": total_rejects,
            "edited": total_edited,
            "items": items,
        }),
        &human,
    );
    Ok(exit_code::OK)
}

enum ResolvedWatchStyle {
    Builtin(String, Box<DevelopRecipe>),
    User(Box<UserStyle>),
}

impl ResolvedWatchStyle {
    fn display_name(&self) -> &str {
        match self {
            Self::Builtin(id, _) => id,
            Self::User(u) => &u.name,
        }
    }
}

fn resolve_watch_style(raw: &str) -> Result<ResolvedWatchStyle, CliError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CliError::validation("--style cannot be empty."));
    }
    if let Ok(recipe) = resolve_builtin_preset(trimmed) {
        return Ok(ResolvedWatchStyle::Builtin(
            trimmed.to_owned(),
            Box::new(recipe),
        ));
    }
    let styles_dir = default_styles_dir();
    if let Ok(style) = load_user_style_in_dir(&styles_dir, trimmed) {
        return Ok(ResolvedWatchStyle::User(Box::new(style)));
    }
    Err(CliError::validation(format!(
        "Unknown --style '{trimmed}' (not a built-in preset or saved User Style)."
    )))
}

fn ingest_hot_folder_photo(
    path: &Path,
    style: Option<&ResolvedWatchStyle>,
    opts: &RuntimeOpts,
) -> Result<Value, CliError> {
    let mut meta = PhotoMeta::load_sidecar(path)?.unwrap_or_default();
    let mut style_applied: Option<String> = None;

    if let Some(st) = style {
        let before = meta.recipe.clone();
        match st {
            ResolvedWatchStyle::Builtin(_, preset) => {
                let mut target = (**preset).clone();
                target.geometry = meta.recipe.geometry;
                meta.recipe = blend_recipes(&meta.recipe, &target, 1.0);
                meta.recipe.preset_name = preset.preset_name.clone();
            }
            ResolvedWatchStyle::User(u) => {
                apply_style_to_recipe(&mut meta.recipe, u, 1.0)?;
            }
        }
        meta.recipe = meta.recipe.sanitized();
        let diffs = recipe_field_diffs(&before, &meta.recipe);
        if !opts.dry_run && !diffs.is_empty() {
            meta.push_history(
                EditActor::Agent {
                    name: "hot-folder".to_owned(),
                    job_id: None,
                },
                format!("Hot Folder auto-ingest style '{}'", st.display_name()),
                diffs,
                Some(before),
            );
            meta.save_sidecar_in_place(path)?;
        }
        style_applied = Some(st.display_name().to_owned());
    }

    let sidecar_path = DevelopRecipe::sidecar_path_for(path);
    Ok(json!({
        "path": path,
        "sidecar": sidecar_path,
        "has_sidecar": sidecar_path.exists(),
        "style_applied": style_applied,
        "revision": meta.revision,
    }))
}

fn cmd_session_init(dir: &Path, opts: &RuntimeOpts) -> Result<u8, CliError> {
    if dir.as_os_str().is_empty() {
        return Err(CliError::validation(
            "Session directory path cannot be empty.",
        ));
    }
    let layout = if opts.dry_run {
        rune_editor_core::SessionFolderLayout {
            root: dir.to_path_buf(),
            capture: dir.join("Capture"),
            selects: dir.join("Selects"),
            output: dir.join("Output"),
            trash: dir.join("Trash"),
        }
    } else {
        create_capture_one_session(dir)?
    };
    let data = json!({
        "root": layout.root,
        "capture": layout.capture,
        "selects": layout.selects,
        "output": layout.output,
        "trash": layout.trash,
        "dry_run": opts.dry_run,
    });
    let human = format!(
        "{} Capture One 4-folder session at {}\n  Capture: {}\n  Selects: {}\n  Output:  {}\n  Trash:   {}",
        if opts.dry_run {
            "[dry-run] Would initialize"
        } else {
            "Initialized"
        },
        layout.root.display(),
        layout.capture.display(),
        layout.selects.display(),
        layout.output.display(),
        layout.trash.display(),
    );
    emit_ok(opts, "session.init", data, &human);
    Ok(exit_code::OK)
}

fn cmd_session_watch(
    dir: &Path,
    style: Option<&str>,
    once: bool,
    interval_ms: u64,
    opts: &RuntimeOpts,
) -> Result<u8, CliError> {
    ensure_exists(dir)?;
    if !dir.is_dir() {
        return Err(CliError::validation(format!(
            "Watch path is not a directory: {}",
            dir.display()
        )));
    }
    let resolved_style = match style {
        Some(s) => Some(resolve_watch_style(s)?),
        None => None,
    };

    if once {
        let mut watcher = HotFolderWatcher::new_empty(dir.to_path_buf())?;
        // Two consecutive polls verify non-zero stable byte sizes without decoding mid-write files.
        let mut ready = watcher.poll_ready()?;
        std::thread::sleep(std::time::Duration::from_millis(interval_ms.clamp(10, 50)));
        ready.extend(watcher.poll_ready()?);

        let mut items = Vec::with_capacity(ready.len());
        for photo in &ready {
            let item = ingest_hot_folder_photo(photo, resolved_style.as_ref(), opts)?;
            if matches!(opts.format, ResolvedFormat::Ndjson) {
                emit_ndjson_line(&item);
            } else {
                items.push(item);
            }
        }
        if matches!(opts.format, ResolvedFormat::Ndjson) {
            return Ok(exit_code::OK);
        }
        let human = format!(
            "Hot-folder single pass on {}: ingested {} photo(s){}",
            dir.display(),
            items.len(),
            resolved_style
                .as_ref()
                .map(|s| format!(" (style '{}')", s.display_name()))
                .unwrap_or_default(),
        );
        emit_ok(
            opts,
            "session.watch",
            json!({
                "dir": dir,
                "once": true,
                "style": resolved_style.as_ref().map(|s| s.display_name()),
                "ingested_count": items.len(),
                "items": items,
            }),
            &human,
        );
        return Ok(exit_code::OK);
    }

    let mut watcher = HotFolderWatcher::new(dir.to_path_buf())?;
    let sleep_dur = std::time::Duration::from_millis(interval_ms.clamp(50, 60_000));
    if !opts.quiet {
        eprintln!(
            "Watching hot folder {} (interval {}ms, Ctrl+C to stop)...",
            dir.display(),
            sleep_dur.as_millis()
        );
    }
    loop {
        let ready = watcher.poll_ready()?;
        for photo in ready {
            let item = ingest_hot_folder_photo(&photo, resolved_style.as_ref(), opts)?;
            if matches!(opts.format, ResolvedFormat::Ndjson | ResolvedFormat::Json) {
                emit_ndjson_line(&item);
            } else if !opts.quiet {
                println!(
                    "Ingested {}{}",
                    photo.display(),
                    resolved_style
                        .as_ref()
                        .map(|s| format!(" [style: {}]", s.display_name()))
                        .unwrap_or_default(),
                );
            }
        }
        std::thread::sleep(sleep_dur);
    }
}

fn is_profile_exif_highlighted(
    desc: &CameraProfileDescriptor,
    idx: usize,
    first_builtin_idx: usize,
    camera_make: Option<&str>,
    camera_model: Option<&str>,
) -> bool {
    let has_filter = camera_make.is_some_and(|s| !s.trim().is_empty())
        || camera_model.is_some_and(|s| !s.trim().is_empty());
    if !has_filter {
        return false;
    }
    if desc.category == ProfileCategory::User {
        return idx < first_builtin_idx;
    }
    if let Some(cm) = desc.camera_match.as_deref()
        && cm != "*"
    {
        let needle = cm.trim().to_ascii_lowercase();
        let make_hit = camera_make
            .map(str::to_ascii_lowercase)
            .is_some_and(|m| m.contains(&needle));
        let model_hit = camera_model
            .map(str::to_ascii_lowercase)
            .is_some_and(|m| m.contains(&needle) || (needle == "apple" && m.contains("iphone")));
        return make_hit || model_hit;
    }
    false
}

fn dry_run_validate_profile_import(
    file: &Path,
    name_override: Option<&str>,
) -> Result<CameraProfileDescriptor, CliError> {
    let meta = std::fs::symlink_metadata(file).map_err(|e| CliError::io(e.to_string()))?;
    if meta.file_type().is_symlink() {
        return Err(CliError::validation(format!(
            "Refusing symlink profile path '{}'",
            file.display()
        )));
    }
    if !meta.file_type().is_file() {
        return Err(CliError::validation(format!(
            "Profile path '{}' is not a regular file",
            file.display()
        )));
    }
    if meta.len() > MAX_PROFILE_FILE_BYTES as u64 {
        return Err(CliError::validation(format!(
            "Profile file '{}' exceeds {} MiB limit",
            file.display(),
            MAX_PROFILE_FILE_BYTES / (1024 * 1024)
        )));
    }
    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| {
            CliError::validation(format!(
                "Profile '{}' is missing a valid extension (.dcp, .cube, .icc, .icm)",
                file.display()
            ))
        })?;
    if !ALLOWED_PROFILE_EXTENSIONS.contains(&ext.as_str()) {
        return Err(CliError::validation(format!(
            "Disallowed profile extension '.{ext}' (allowed: .dcp, .cube, .icc, .icm)"
        )));
    }
    let bytes = std::fs::read(file).map_err(|e| CliError::io(e.to_string()))?;
    let (display_name, camera_match, tag) = match ext.as_str() {
        "cube" => {
            let c = parse_cube_lut(&bytes)?;
            (c.title, None, "CUBE")
        }
        "dcp" => {
            let d = parse_dcp_profile(&bytes)?;
            (d.profile_name, d.unique_camera_model, "DCP")
        }
        "icc" | "icm" => {
            let i = parse_icc_profile(&bytes)?;
            (i.description.clone(), i.description, "ICC")
        }
        _ => unreachable!("checked against ALLOWED_PROFILE_EXTENSIONS"),
    };
    let stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("profile");
    let chosen_name = name_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or(display_name.as_deref())
        .unwrap_or(stem)
        .to_owned();
    let slug = sanitize_profile_slug(&chosen_name)?;
    Ok(CameraProfileDescriptor {
        id: format!("{USER_PROFILE_PREFIX}{slug}"),
        name: chosen_name,
        category: ProfileCategory::User,
        description: format!("Imported {tag} camera profile ({slug}.{ext})"),
        camera_match,
    })
}

fn cmd_profile(sub: ProfileCmd, opts: &RuntimeOpts) -> Result<u8, CliError> {
    match sub {
        ProfileCmd::List {
            camera,
            make,
            profiles_dir,
        } => {
            let dir = profiles_dir.clone().unwrap_or_else(default_profiles_dir);
            let profiles = list_all_camera_profiles(
                make.as_deref(),
                camera.as_deref(),
                profiles_dir.as_deref(),
            );
            let first_builtin_idx = profiles
                .iter()
                .position(|p| p.category != ProfileCategory::User)
                .unwrap_or(profiles.len());

            let items: Vec<Value> = profiles
                .iter()
                .enumerate()
                .map(|(idx, p)| {
                    let exif_matched = is_profile_exif_highlighted(
                        p,
                        idx,
                        first_builtin_idx,
                        make.as_deref(),
                        camera.as_deref(),
                    );
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

            let cal_count = profiles
                .iter()
                .filter(|p| p.category == ProfileCategory::Calibration)
                .count();
            let film_count = profiles
                .iter()
                .filter(|p| p.category == ProfileCategory::FilmSimulation)
                .count();
            let user_count = profiles
                .iter()
                .filter(|p| p.category == ProfileCategory::User)
                .count();

            let mut lines = Vec::new();
            let render_group = |lines: &mut Vec<String>, title: &str, cat: ProfileCategory| {
                let group: Vec<(usize, &CameraProfileDescriptor)> = profiles
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.category == cat)
                    .collect();
                if group.is_empty() {
                    return;
                }
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                lines.push(format!("{title} ({}):", group.len()));
                for (idx, p) in group {
                    let matched = is_profile_exif_highlighted(
                        p,
                        idx,
                        first_builtin_idx,
                        make.as_deref(),
                        camera.as_deref(),
                    );
                    let match_badge = match (matched, p.camera_match.as_deref()) {
                        (true, Some(cm)) => format!(" ★ MATCH [{cm}]"),
                        (true, None) => " ★ MATCH".to_owned(),
                        (false, Some(cm)) => format!(" [{cm}]"),
                        (false, None) => String::new(),
                    };
                    lines.push(format!(
                        "  {:24} {}{} — {}",
                        p.id, p.name, match_badge, p.description
                    ));
                }
            };
            render_group(
                &mut lines,
                "Camera Calibration",
                ProfileCategory::Calibration,
            );
            render_group(
                &mut lines,
                "Film Simulations",
                ProfileCategory::FilmSimulation,
            );
            render_group(&mut lines, "User Profiles", ProfileCategory::User);

            emit_ok(
                opts,
                "profile.list",
                json!({
                    "profiles_dir": dir,
                    "camera_make": make,
                    "camera_model": camera,
                    "count": items.len(),
                    "calibration_count": cal_count,
                    "film_simulation_count": film_count,
                    "user_count": user_count,
                    "profiles": items,
                }),
                &lines.join("\n"),
            );
            Ok(exit_code::OK)
        }
        ProfileCmd::Import {
            file,
            name,
            overwrite,
            profiles_dir,
        } => {
            ensure_exists(&file)?;
            let dir = profiles_dir.clone().unwrap_or_else(default_profiles_dir);
            let desc = if opts.dry_run {
                dry_run_validate_profile_import(&file, name.as_deref())?
            } else if profiles_dir.is_none() && !overwrite {
                import_user_profile(&file, name.as_deref())?
            } else {
                import_user_profile_in_dir_with_overwrite(&dir, &file, name.as_deref(), overwrite)?
            };
            let match_note = desc
                .camera_match
                .as_deref()
                .map(|m| format!(" [camera_match: {m}]"))
                .unwrap_or_default();
            let human = format!(
                "{} profile '{}' ({}){} -> {}",
                if opts.dry_run {
                    "[dry-run] Would import"
                } else {
                    "Imported"
                },
                desc.id,
                desc.name,
                match_note,
                dir.display(),
            );
            emit_ok(
                opts,
                "profile.import",
                json!({
                    "id": desc.id,
                    "name": desc.name,
                    "category": desc.category,
                    "description": desc.description,
                    "camera_match": desc.camera_match,
                    "profile": desc,
                    "profiles_dir": dir,
                    "overwrite": overwrite,
                }),
                &human,
            );
            Ok(exit_code::OK)
        }
        ProfileCmd::Delete { slug, profiles_dir } => {
            let dir = profiles_dir.clone().unwrap_or_else(default_profiles_dir);
            let bare = slug
                .trim()
                .strip_prefix(USER_PROFILE_PREFIX)
                .unwrap_or(slug.trim());
            let clean_slug = sanitize_profile_slug(bare)?;
            let deleted = if opts.dry_run {
                ALLOWED_PROFILE_EXTENSIONS
                    .iter()
                    .any(|ext| dir.join(format!("{clean_slug}.{ext}")).exists())
            } else if profiles_dir.is_none() {
                delete_user_profile(&slug)?
            } else {
                delete_user_profile_in_dir(&dir, &slug)?
            };
            if !deleted {
                return Err(CliError::not_found(format!(
                    "User profile '{USER_PROFILE_PREFIX}{clean_slug}' not found in {}",
                    dir.display()
                )));
            }
            let human = format!(
                "{} user profile '{USER_PROFILE_PREFIX}{clean_slug}'",
                if opts.dry_run {
                    "[dry-run] Would delete"
                } else {
                    "Deleted"
                }
            );
            emit_ok(
                opts,
                "profile.delete",
                json!({
                    "slug": clean_slug,
                    "id": format!("{USER_PROFILE_PREFIX}{clean_slug}"),
                    "deleted": true,
                    "profiles_dir": dir,
                }),
                &human,
            );
            Ok(exit_code::OK)
        }
    }
}

fn cmd_preset(sub: PresetCmd, opts: &RuntimeOpts) -> Result<u8, CliError> {
    match sub {
        PresetCmd::List => {
            let list: Vec<Value> = builtin_presets()
                .into_iter()
                .map(|p| {
                    json!({
                        "id": p.id,
                        "name": p.name,
                        "overrides": recipe_non_default_map(&p.recipe),
                    })
                })
                .collect();
            let human = list
                .iter()
                .map(|p| {
                    format!(
                        "  {:16} {}",
                        p["id"].as_str().unwrap_or(""),
                        p["name"].as_str().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            emit_ok(opts, "preset.list", json!({ "presets": list }), &human);
            Ok(exit_code::OK)
        }
        PresetCmd::Show { id } => {
            let recipe = load_preset_or_file(&id)?;
            let human = serde_json::to_string_pretty(&recipe).unwrap_or_default();
            emit_ok(opts, "preset.show", json!(recipe), &human);
            Ok(exit_code::OK)
        }
        PresetCmd::Apply {
            id,
            paths,
            mix,
            only,
        } => {
            let edit_args = EditArgs {
                paths,
                recursive: false,
                picks: false,
                min_rating: None,
                sets: Vec::new(),
                preset: Some(id),
                mix,
                auto: Vec::new(),
                wb_pick: None,
                wb_preset: None,
                profile: None,
                profile_strength: None,
                film_curve: None,
                processing_mode: None,
                pick_film_base: None,
                pick_skin: None,
                pick_color_slices: Vec::new(),
                vignette_method: None,
                copy_from: None,
                only,
                patch: None,
                reset: None,
                exposure: None,
                contrast: None,
                highlights: None,
                shadows: None,
                temp: None,
                tint: None,
                vibrance: None,
                saturation: None,
                clarity: None,
                structure: None,
                dehaze: None,
                sharpen: None,
                denoise: None,
                aspect: None,
                agent: "preset".to_owned(),
                force: false,
            };
            cmd_edit(edit_args, opts)
        }
        PresetCmd::Save { photo, out, name } => {
            ensure_exists(&photo)?;
            let mut recipe = PhotoMeta::load_sidecar(&photo)?.unwrap_or_default().recipe;
            if let Some(n) = name {
                recipe.preset_name = Some(n);
            }
            if !opts.dry_run {
                let text = serde_json::to_string_pretty(&recipe)
                    .map_err(|e| CliError::io(e.to_string()))?;
                rune_editor_core::session::atomic_write_bytes(&out, text.as_bytes())?;
            }
            let human = format!("Saved preset to {}", out.display());
            emit_ok(
                opts,
                "preset.save",
                json!({ "source": photo, "out": out }),
                &human,
            );
            Ok(exit_code::OK)
        }
    }
}

fn cmd_preview(args: PreviewArgs, opts: &RuntimeOpts) -> Result<u8, CliError> {
    ensure_exists(&args.photo)?;
    let start = Instant::now();
    let meta = PhotoMeta::load_sidecar(&args.photo)?.unwrap_or_default();
    let max_edge = args.max_edge.max(64);
    let img = load_file_proxy(&args.photo, Some(max_edge))?;

    let format = infer_format_from_ext(&args.out, args.quality);
    if !opts.dry_run {
        guard_not_source(&args.photo, &args.out)?;
        if !args.overwrite && (args.out.exists() || args.out.symlink_metadata().is_ok()) {
            return Err(CliError::validation(format!(
                "Refusing to overwrite existing file '{}' without `--overwrite`.",
                args.out.display()
            )));
        }
        export_image_with_options(
            &img,
            &meta.recipe,
            &args.out,
            &ExportOptions {
                format,
                max_edge: Some(max_edge),
                respect_crop: !args.no_crop,
                output_sharpen: 0.15,
                ..ExportOptions::default()
            },
        )?;
    }

    let elapsed_ms = start.elapsed().as_millis();
    let hist = compute_histogram(&img.histogram_proxy, &meta.recipe);
    let data = json!({
        "photo": args.photo,
        "out": args.out,
        "max_edge": max_edge,
        "quality": args.quality,
        "elapsed_ms": elapsed_ms,
        "telemetry": {
            "shadow_clip_pct": (hist.shadow_clip_pct * 100.0).round() / 100.0,
            "highlight_clip_pct": (hist.highlight_clip_pct * 100.0).round() / 100.0,
        },
        "recipe_overrides": recipe_non_default_map(&meta.recipe),
    });
    let human = format!(
        "Rendered preview {} ({}ms, clip S={:.2}% H={:.2}%)",
        args.out.display(),
        elapsed_ms,
        hist.shadow_clip_pct,
        hist.highlight_clip_pct,
    );
    emit_ok(opts, "preview", data, &human);
    Ok(exit_code::OK)
}

fn cmd_diff(args: DiffArgs, opts: &RuntimeOpts) -> Result<u8, CliError> {
    ensure_exists(&args.photo)?;
    let proxy = load_file_proxy(&args.photo, Some(args.proxy_edge.max(64)))?;
    let current = PhotoMeta::load_sidecar(&args.photo)?
        .unwrap_or_default()
        .recipe;

    let baseline = if let Some(preset_id) = &args.against_preset {
        load_preset_or_file(preset_id)?
    } else if let Some(other) = &args.baseline {
        ensure_exists(other)?;
        PhotoMeta::load_sidecar(other)?.unwrap_or_default().recipe
    } else {
        DevelopRecipe::default()
    };

    let diff = diff_recipes(&proxy.histogram_proxy, &baseline, &current);
    let analysis = analyze_linear_proxy(&proxy.histogram_proxy, &current, None);

    let mut violations = Vec::new();
    for rule in &args.assertions {
        let (k, raw_v) = rule.split_once('=').ok_or_else(|| {
            CliError::validation(format!("Invalid --assert '{rule}' (expected key=value)"))
        })?;
        let limit: f32 = raw_v
            .trim()
            .parse()
            .map_err(|_| CliError::validation(format!("Invalid number in --assert '{rule}'")))?;
        let (actual, ok) = match k.trim() {
            "max_highlight_clip_pct" => (
                analysis.tone.clip_pct.highlight_any,
                analysis.tone.clip_pct.highlight_any <= limit,
            ),
            "max_shadow_clip_pct" => (
                analysis.tone.clip_pct.shadow_all,
                analysis.tone.clip_pct.shadow_all <= limit,
            ),
            "max_skin_de2000" => (
                diff.delta_e2000.skin_tones,
                diff.delta_e2000.skin_tones <= limit,
            ),
            "max_mean_de2000" => (diff.delta_e2000.mean, diff.delta_e2000.mean <= limit),
            other => {
                return Err(CliError::validation(format!(
                    "Unknown --assert metric '{other}'. Valid: max_highlight_clip_pct, max_shadow_clip_pct, max_skin_de2000, max_mean_de2000"
                )));
            }
        };
        if !ok {
            violations.push(format!("{k}: actual {actual:.2} > limit {limit:.2}"));
        }
    }

    let passed = violations.is_empty();
    let data = json!({
        "photo": args.photo,
        "diff": diff,
        "assertions_passed": passed,
        "violations": violations,
    });
    let human = format!(
        "Diff {}: ΔE2000 mean={:.2} p95={:.2} skin={:.2} | ΔEV={:+.2} | Clip ΔH={:+.2}% | Changes={}{}",
        args.photo.display(),
        diff.delta_e2000.mean,
        diff.delta_e2000.p95,
        diff.delta_e2000.skin_tones,
        diff.luma_shift.mean_delta_ev,
        diff.clip_delta_pct.highlight_any,
        diff.recipe_changes.len(),
        if passed {
            String::new()
        } else {
            format!("\n  ASSERT FAILED: {}", violations.join("; "))
        }
    );

    if !passed {
        if matches!(opts.format, ResolvedFormat::Json | ResolvedFormat::Ndjson) {
            println!(
                "{}",
                json!({ "ok": false, "command": "diff", "data": data })
            );
            return Ok(exit_code::PARTIAL_OR_ASSERT);
        }
        return Err(CliError::assertion(violations.join("; ")));
    }

    emit_ok(opts, "diff", data, &human);
    Ok(exit_code::OK)
}

fn cmd_export(args: ExportArgs, opts: &RuntimeOpts) -> Result<u8, CliError> {
    let mut files = expand_input_paths(&args.paths, args.recursive, &[])?;
    if files.is_empty() {
        return Err(CliError::not_found("No supported photos found for export."));
    }
    if args.picks || args.min_rating.is_some() {
        files.retain(|p| {
            let m = PhotoMeta::load_sidecar(p)
                .ok()
                .flatten()
                .unwrap_or_default();
            let ok_pick = !args.picks || m.flag == CullFlag::Pick;
            let ok_rate = args.min_rating.is_none_or(|min| m.rating >= min);
            ok_pick && ok_rate
        });
    }

    let (format, ext) = parse_export_format(&args.to, args.quality)?;
    let color_space = ExportColorSpace::from_cli_str(&args.color_space).ok_or_else(|| {
        CliError::validation(format!(
            "Unsupported --color-space '{}' (expected srgb or display-p3)",
            args.color_space
        ))
    })?;
    let wm_position = WatermarkPosition::from_cli_str(&args.watermark_position).ok_or_else(|| {
        CliError::validation(format!(
            "Unsupported --watermark-position '{}' (expected bottom_right|bottom_left|top_right|top_left|center)",
            args.watermark_position
        ))
    })?;
    if !args.watermark_opacity.is_finite() || !(0.0..=1.0).contains(&args.watermark_opacity) {
        return Err(CliError::validation(format!(
            "--watermark-opacity must be within 0.0..=1.0 (got {})",
            args.watermark_opacity
        )));
    }
    if !args.watermark_scale.is_finite() || !(0.5..=3.0).contains(&args.watermark_scale) {
        return Err(CliError::validation(format!(
            "--watermark-scale must be within 0.5..=3.0 (got {})",
            args.watermark_scale
        )));
    }
    let watermark = match &args.watermark {
        Some(text) if !text.trim().is_empty() => TextWatermark {
            enabled: true,
            text: text.trim().to_owned(),
            opacity: args.watermark_opacity,
            scale: args.watermark_scale,
            position: wm_position,
        },
        _ => TextWatermark::default(),
    };
    let export_opts = ExportOptions {
        format,
        max_edge: args.max_edge,
        respect_crop: !args.no_crop,
        output_sharpen: args.output_sharpen,
        strip_metadata: args.strip_metadata,
        watermark,
        color_space,
    };

    if let Some(recipes_raw) = &args.recipes {
        return run_multi_recipe_export(&files, &args, recipes_raw, &export_opts, opts);
    }

    let single_file_dest = files.len() == 1
        && args.naming_template.is_none()
        && !args.out.is_dir()
        && args.out.extension().is_some();

    let mut pairs: Vec<(PathBuf, PathBuf)> = Vec::with_capacity(files.len());
    for (idx, src) in files.iter().enumerate() {
        let dest = if single_file_dest {
            args.out.clone()
        } else if let Some(tpl) = &args.naming_template {
            let meta = PhotoMeta::load_sidecar(src)
                .ok()
                .flatten()
                .unwrap_or_default();
            let ctx = build_naming_context(src, idx + 1, &meta, &args.to);
            let stem = expand_naming_template(tpl, &ctx);
            args.out.join(format!("{stem}.{ext}"))
        } else {
            render_dest_path(src, &args.out, &args.naming, ext)?
        };
        pairs.push((src.clone(), dest));
    }
    check_duplicate_export_paths(&pairs)?;

    let jobs = args.jobs.clamp(1, 8);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .build()
        .map_err(|e| CliError::io(e.to_string()))?;

    let start = Instant::now();

    if matches!(opts.format, ResolvedFormat::Ndjson) {
        emit_ndjson_line(&json!({
            "type": "plan",
            "total": pairs.len(),
            "jobs": jobs,
            "format": args.to,
            "dry_run": opts.dry_run,
        }));
    }

    let results: Vec<Value> = pool.install(|| {
        pairs
            .par_iter()
            .enumerate()
            .map(|(idx, (src, dest))| {
                let item_start = Instant::now();

                if let Err(err) = guard_not_source(src, dest) {
                    return json!({
                        "type": "error",
                        "index": idx + 1,
                        "path": src,
                        "dest": dest,
                        "code": err.code,
                        "message": err.message,
                    });
                }

                if dest.exists() && args.skip_existing {
                    return json!({
                        "type": "item",
                        "index": idx + 1,
                        "path": src,
                        "dest": dest,
                        "status": "skipped",
                    });
                }
                if dest.exists() && !args.overwrite {
                    return json!({
                        "type": "error",
                        "index": idx + 1,
                        "path": src,
                        "dest": dest,
                        "code": "OUTPUT_EXISTS",
                        "message": format!("Destination '{}' exists (pass --overwrite or --skip-existing)", dest.display()),
                    });
                }

                if opts.dry_run {
                    return json!({
                        "type": "item",
                        "index": idx + 1,
                        "path": src,
                        "dest": dest,
                        "status": "dry_run",
                    });
                }

                let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), CliError> {
                    let meta = PhotoMeta::load_sidecar(src)?.unwrap_or_default();
                    let img = load_file(src)?;
                    export_image_with_options(&img, &meta.recipe, dest, &export_opts)?;
                    Ok(())
                }));

                match run {
                    Ok(Ok(())) => json!({
                        "type": "item",
                        "index": idx + 1,
                        "path": src,
                        "dest": dest,
                        "status": "ok",
                        "ms": item_start.elapsed().as_millis(),
                    }),
                    Ok(Err(err)) => json!({
                        "type": "error",
                        "index": idx + 1,
                        "path": src,
                        "dest": dest,
                        "code": err.code,
                        "message": err.message,
                    }),
                    Err(_) => json!({
                        "type": "error",
                        "index": idx + 1,
                        "path": src,
                        "dest": dest,
                        "code": "DECODE_PANIC",
                        "message": "Unexpected decoder panic isolated for this file",
                    }),
                }
            })
            .collect()
    });

    let mut succeeded = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    for r in &results {
        if r["type"] == "error" {
            failed += 1;
        } else if r["status"] == "skipped" {
            skipped += 1;
        } else {
            succeeded += 1;
        }
        if matches!(opts.format, ResolvedFormat::Ndjson) {
            emit_ndjson_line(r);
        }
    }

    let elapsed_ms = start.elapsed().as_millis();
    let code = if failed == 0 {
        exit_code::OK
    } else {
        exit_code::PARTIAL_OR_ASSERT
    };

    if matches!(opts.format, ResolvedFormat::Ndjson) {
        emit_ndjson_line(&json!({
            "type": "summary",
            "ok": failed == 0,
            "exit_code": code,
            "total": files.len(),
            "succeeded": succeeded,
            "skipped": skipped,
            "failed": failed,
            "elapsed_ms": elapsed_ms,
        }));
        return Ok(code);
    }

    let human = format!(
        "Exported {succeeded}/{} photo(s) ({skipped} skipped, {failed} failed) in {elapsed_ms}ms",
        files.len()
    );
    emit_ok(
        opts,
        "export",
        json!({
            "total": files.len(),
            "succeeded": succeeded,
            "skipped": skipped,
            "failed": failed,
            "elapsed_ms": elapsed_ms,
            "items": results,
        }),
        &human,
    );
    Ok(code)
}

fn cmd_batch_run(manifest_arg: &str, jobs: usize, opts: &RuntimeOpts) -> Result<u8, CliError> {
    let text = if manifest_arg == "-" {
        read_arg_or_stdin("-")?
    } else {
        std::fs::read_to_string(manifest_arg).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                CliError::not_found(format!("Manifest not found: {manifest_arg}"))
            } else {
                CliError::io(e.to_string())
            }
        })?
    };

    let job: BatchJob = serde_json::from_str(&text)
        .map_err(|e| CliError::validation(format!("Invalid BatchJob JSON: {e}")))?;

    let mut files = expand_input_paths(&job.inputs.paths, job.inputs.recursive, &[])?;
    if job.inputs.picks_only || job.inputs.min_rating.is_some() {
        files.retain(|p| {
            let m = PhotoMeta::load_sidecar(p)
                .ok()
                .flatten()
                .unwrap_or_default();
            let ok_pick = !job.inputs.picks_only || m.flag == CullFlag::Pick;
            let ok_rate = job.inputs.min_rating.is_none_or(|min| m.rating >= min);
            ok_pick && ok_rate
        });
    }

    if !job.exports.is_empty() {
        let mut planned_pairs: Vec<(PathBuf, PathBuf)> =
            Vec::with_capacity(files.len() * job.exports.len());
        for src in &files {
            for target in &job.exports {
                let (_fmt, ext) = parse_export_format(&target.format, target.quality)?;
                let dest =
                    render_dest_path(src, &target.output_dir, &target.filename_template, ext)?;
                guard_not_source(src, &dest)?;
                // Prevent overwriting any neighbouring original photo in the batch (e.g. RAW+JPEG pairs).
                for other_src in &files {
                    guard_not_source(other_src, &dest)?;
                }
                if !opts.dry_run
                    && !target.overwrite
                    && (dest.exists() || dest.symlink_metadata().is_ok())
                {
                    return Err(CliError::validation(format!(
                        "Refusing to overwrite existing file '{}' for export target '{}' (set `overwrite: true` to replace).",
                        dest.display(),
                        target.id
                    )));
                }
                planned_pairs.push((src.clone(), dest));
            }
        }
        check_duplicate_export_paths(&planned_pairs)?;
    }

    let start = Instant::now();
    let actor_name = job.agent.clone().unwrap_or_else(|| "batch".to_owned());
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs.clamp(1, 8))
        .build()
        .map_err(|e| CliError::io(e.to_string()))?;

    if matches!(opts.format, ResolvedFormat::Ndjson) {
        emit_ndjson_line(&json!({
            "type": "plan",
            "job_id": job.job_id,
            "total": files.len(),
            "rules": job.pipeline.len(),
            "export_targets": job.exports.len(),
            "dry_run": opts.dry_run,
        }));
    }

    let items: Vec<Value> = pool.install(|| {
        files
            .par_iter()
            .enumerate()
            .map(|(idx, src)| {
                let t0 = Instant::now();
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_batch_item(src, &job, &actor_name, opts)
                }));
                match res {
                    Ok(Ok(val)) => {
                        let mut v = val;
                        v["type"] = json!("item");
                        v["index"] = json!(idx + 1);
                        v["ms"] = json!(t0.elapsed().as_millis());
                        v
                    }
                    Ok(Err(err)) => json!({
                        "type": "error",
                        "index": idx + 1,
                        "path": src,
                        "code": err.code,
                        "message": err.message,
                    }),
                    Err(_) => json!({
                        "type": "error",
                        "index": idx + 1,
                        "path": src,
                        "code": "BATCH_ITEM_PANIC",
                        "message": "Unexpected panic isolated for batch item",
                    }),
                }
            })
            .collect()
    });

    let failed = items.iter().filter(|v| v["type"] == "error").count();
    let succeeded = items.len() - failed;
    let code = if failed == 0 {
        exit_code::OK
    } else {
        exit_code::PARTIAL_OR_ASSERT
    };

    if matches!(opts.format, ResolvedFormat::Ndjson) {
        for item in &items {
            emit_ndjson_line(item);
        }
        emit_ndjson_line(&json!({
            "type": "summary",
            "ok": failed == 0,
            "exit_code": code,
            "total": files.len(),
            "succeeded": succeeded,
            "failed": failed,
            "elapsed_ms": start.elapsed().as_millis(),
        }));
        return Ok(code);
    }

    let human = format!(
        "Batch complete: {succeeded}/{} succeeded ({failed} failed) in {}ms",
        files.len(),
        start.elapsed().as_millis()
    );
    emit_ok(
        opts,
        "batch.run",
        json!({
            "total": files.len(),
            "succeeded": succeeded,
            "failed": failed,
            "items": items,
        }),
        &human,
    );
    Ok(code)
}

fn run_batch_item(
    src: &Path,
    job: &BatchJob,
    actor_name: &str,
    opts: &RuntimeOpts,
) -> Result<Value, CliError> {
    let loaded = PhotoMeta::load_sidecar(src)?;
    let had_sidecar = loaded.is_some();
    let mut meta = loaded.unwrap_or_default();
    let before = meta.recipe.clone();
    let rating_before = meta.rating;
    let flag_before = meta.flag;
    let color_tag_before = meta.color_tag;
    let exif = extract_exif_from_path(src);

    let mut proxy_cache = None;
    let mut dims_cache = None;
    let mut matched_rules = Vec::new();

    for rule in &job.pipeline {
        let should_run = match &rule.when {
            None => true,
            Some(expr) => eval_condition(expr, src, &meta, exif.as_ref(), &mut proxy_cache)?,
        };
        if !should_run {
            continue;
        }
        if let Some(name) = &rule.name {
            matched_rules.push(name.clone());
        }
        for op in &rule.ops {
            apply_edit_op(op, src, &mut meta, &mut proxy_cache, &mut dims_cache)?;
        }
    }

    meta.recipe = meta.recipe.sanitized();
    let diffs = recipe_field_diffs(&before, &meta.recipe);
    let meta_changed = meta.rating != rating_before
        || meta.flag != flag_before
        || meta.color_tag != color_tag_before;

    if !opts.dry_run && (!diffs.is_empty() || meta_changed) {
        match job.sidecar_mode {
            rune_editor_core::patch::SidecarWriteMode::Update => {
                meta.push_history(
                    EditActor::Agent {
                        name: actor_name.to_owned(),
                        job_id: job.job_id.clone(),
                    },
                    format!("batch: {}", matched_rules.join(", ")),
                    diffs.clone(),
                    (!diffs.is_empty()).then_some(before),
                );
                meta.save_sidecar_in_place(src)?;
            }
            rune_editor_core::patch::SidecarWriteMode::CreateIfMissing if !had_sidecar => {
                meta.save_sidecar_in_place(src)?;
            }
            _ => {}
        }
    }

    let mut outputs = Vec::new();
    if !job.exports.is_empty() {
        let full_img = if opts.dry_run {
            None
        } else {
            Some(load_file(src)?)
        };
        for target in &job.exports {
            let (fmt, ext) = parse_export_format(&target.format, target.quality)?;
            let dest = render_dest_path(src, &target.output_dir, &target.filename_template, ext)?;
            guard_not_source(src, &dest)?;
            if !opts.dry_run
                && !target.overwrite
                && (dest.exists() || dest.symlink_metadata().is_ok())
            {
                return Err(CliError::validation(format!(
                    "Refusing to overwrite existing file '{}' for export target '{}' (set `overwrite: true` to replace).",
                    dest.display(),
                    target.id
                )));
            }
            if let Some(img) = &full_img {
                export_image_with_options(
                    img,
                    &meta.recipe,
                    &dest,
                    &ExportOptions {
                        format: fmt,
                        max_edge: target.max_edge,
                        respect_crop: target.respect_crop,
                        output_sharpen: target.output_sharpen,
                        ..ExportOptions::default()
                    },
                )?;
            }
            outputs.push(dest);
        }
    }

    Ok(json!({
        "path": src,
        "status": if opts.dry_run { "dry_run" } else { "ok" },
        "matched_rules": matched_rules,
        "changed_count": diffs.len(),
        "outputs": outputs,
    }))
}

fn eval_condition(
    expr: &rune_editor_core::patch::ConditionExpr,
    src: &Path,
    meta: &PhotoMeta,
    exif: Option<&rune_editor_decode::PhotoExif>,
    proxy_cache: &mut Option<rune_editor_decode::LinearImage>,
) -> Result<bool, CliError> {
    use rune_editor_core::patch::ConditionExpr;
    match expr {
        ConditionExpr::All(list) => {
            for c in list {
                if !eval_condition(c, src, meta, exif, proxy_cache)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        ConditionExpr::Any(list) => {
            for c in list {
                if eval_condition(c, src, meta, exif, proxy_cache)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        ConditionExpr::Not(inner) => Ok(!eval_condition(inner, src, meta, exif, proxy_cache)?),
        ConditionExpr::IsoGte(min) => Ok(exif.and_then(|e| e.iso).is_some_and(|i| i >= *min)),
        ConditionExpr::IsoLte(max) => Ok(exif.and_then(|e| e.iso).is_some_and(|i| i <= *max)),
        ConditionExpr::FlagIs(flag) => Ok(meta.flag == *flag),
        ConditionExpr::RatingGte(min) => Ok(meta.rating >= *min),
        ConditionExpr::HighlightClipPctGt(pct) => {
            let p = ensure_proxy(src, proxy_cache)?;
            let hist = compute_histogram(&p.histogram_proxy, &meta.recipe);
            Ok(hist.highlight_clip_pct > *pct)
        }
        ConditionExpr::ShadowClipPctGt(pct) => {
            let p = ensure_proxy(src, proxy_cache)?;
            let hist = compute_histogram(&p.histogram_proxy, &meta.recipe);
            Ok(hist.shadow_clip_pct > *pct)
        }
    }
}

fn apply_edit_op(
    op: &EditOp,
    src: &Path,
    meta: &mut PhotoMeta,
    proxy_cache: &mut Option<rune_editor_decode::LinearImage>,
    dims_cache: &mut Option<[u32; 2]>,
) -> Result<(), CliError> {
    match op {
        EditOp::Reset { keep_geometry } => {
            let geo = meta.recipe.geometry;
            meta.recipe = DevelopRecipe::default();
            if *keep_geometry {
                meta.recipe.geometry = geo;
            }
        }
        EditOp::ApplyPreset {
            id,
            preserve_wb,
            preserve_geometry,
            mix,
        } => {
            let mut target = load_preset_or_file(id)?;
            let target_preset_name = target.preset_name.clone();
            if *preserve_wb {
                target.wb_preset = meta.recipe.wb_preset;
                target.temperature = meta.recipe.temperature;
                target.tint = meta.recipe.tint;
            }
            if *preserve_geometry {
                target.geometry = meta.recipe.geometry;
            }
            meta.recipe = blend_recipes(&meta.recipe, &target, *mix);
            if (*mix - 1.0).abs() <= 1e-4 {
                meta.recipe.preset_name = target_preset_name;
            }
        }
        EditOp::Patch(patch) => {
            let dims = if patch.geometry.is_some() {
                if dims_cache.is_none() {
                    *dims_cache = Some(probe_image_dimensions(src)?);
                }
                *dims_cache
            } else {
                proxy_cache.as_ref().map(|p| [p.width, p.height])
            };
            patch.apply_to(&mut meta.recipe, dims)?;
        }
        EditOp::AutoWb => {
            let p = ensure_proxy(src, proxy_cache)?;
            let (t, ti) = auto_white_balance(&p.histogram_proxy);
            meta.recipe.temperature = t;
            meta.recipe.tint = ti;
            meta.recipe.wb_preset = WbPreset::Custom;
            meta.recipe.preset_name = None;
        }
        EditOp::PickWb { x_norm, y_norm } => {
            let p = ensure_proxy(src, proxy_cache)?;
            let rgb = sample_patch_3x3(p, *x_norm, *y_norm);
            let (t, ti) = pick_white_balance(rgb);
            meta.recipe.temperature = t;
            meta.recipe.tint = ti;
            meta.recipe.wb_preset = WbPreset::Custom;
            meta.recipe.preset_name = None;
        }
        EditOp::AutoTone {
            exposure,
            contrast,
            highlights_shadows,
        } => {
            let p = ensure_proxy(src, proxy_cache)?;
            let (e, c, hi, sh) = auto_exposure_and_tone(&p.histogram_proxy);
            if *exposure {
                meta.recipe.exposure = e;
            }
            if *contrast {
                meta.recipe.contrast = c;
            }
            if *highlights_shadows {
                meta.recipe.highlights = hi;
                meta.recipe.shadows = sh;
            }
            if *exposure || *contrast || *highlights_shadows {
                meta.recipe.preset_name = None;
            }
        }
        EditOp::AutoLevels => {
            let p = ensure_proxy(src, proxy_cache)?;
            meta.recipe.levels = auto_levels(&p.histogram_proxy);
            meta.recipe.preset_name = None;
        }
        EditOp::SetMeta {
            rating,
            flag,
            color_tag,
        } => {
            if let Some(r) = rating {
                meta.rating = (*r).min(5);
            }
            if let Some(f) = flag {
                meta.flag = *f;
            }
            if let Some(t) = color_tag {
                meta.color_tag = *t;
            }
        }
        EditOp::MatchReference(_) => {
            return Err(CliError::validation(
                "Cross-frame `match_reference` solver is scheduled for Phase C.",
            ));
        }
    }
    Ok(())
}

fn ensure_proxy<'a>(
    src: &Path,
    cache: &'a mut Option<rune_editor_decode::LinearImage>,
) -> Result<&'a rune_editor_decode::LinearImage, CliError> {
    if cache.is_none() {
        *cache = Some(load_file_proxy(src, Some(320))?);
    }
    Ok(cache.as_ref().unwrap())
}

fn cmd_mcp(http: bool, port: u16) -> Result<u8, CliError> {
    let service = std::sync::Arc::new(rune_editor_mcp::McpService::new(std::sync::Arc::new(
        rune_editor_mcp::HeadlessHostBridge,
    )));
    if http {
        let server = rune_editor_mcp::LocalMcpServer::start(port, service).map_err(CliError::io)?;
        eprintln!(
            "Rune Darkroom MCP {} listening on {} (127.0.0.1 only, Ctrl+C to stop)",
            rune_editor_mcp::MCP_PROTOCOL_LATEST,
            server.endpoint_url()
        );
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    } else {
        let stdin = io::stdin();
        let stdout = io::stdout();
        rune_editor_mcp::serve_stdio(&service, stdin.lock(), stdout.lock())
            .map_err(|e| CliError::io(e.to_string()))?;
        Ok(exit_code::OK)
    }
}

fn cmd_open(path: Option<PathBuf>, opts: &RuntimeOpts) -> Result<u8, CliError> {
    let canonical_path = match &path {
        Some(p) => {
            ensure_exists(p)?;
            Some(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
        }
        None => None,
    };
    let sibling_gui = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|dir| dir.join("rune-editor")))
        .filter(|p| p.exists());

    let (launched, mut cmd) = if let Some(bin) = sibling_gui {
        (bin.display().to_string(), ProcessCommand::new(&bin))
    } else {
        ("rune-editor".to_owned(), ProcessCommand::new("rune-editor"))
    };
    if let Some(p) = &canonical_path {
        cmd.arg(p);
    }
    if !opts.dry_run {
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| CliError::io(format!("Could not launch `{launched}` GUI binary: {e}")))?;
    }

    let human = format!("Launched {launched}");
    emit_ok(
        opts,
        "open",
        json!({ "binary": launched, "path": canonical_path }),
        &human,
    );
    Ok(exit_code::OK)
}

fn ensure_exists(path: &Path) -> Result<(), CliError> {
    if !path.exists() {
        return Err(CliError::not_found(format!(
            "Path does not exist: {}",
            path.display()
        )));
    }
    Ok(())
}

fn expand_input_paths(
    inputs: &[PathBuf],
    recursive: bool,
    extensions: &[String],
) -> Result<Vec<PathBuf>, CliError> {
    let mut out = Vec::new();
    for p in inputs {
        ensure_exists(p)?;
        if p.is_dir() {
            let scanned = scan_photo_folder_with_options(
                p,
                &ScanOptions {
                    recursive,
                    extensions: extensions.to_vec(),
                },
            )?;
            out.extend(scanned);
        } else if is_supported_photo(p) {
            out.push(p.clone());
        } else {
            return Err(CliError::not_found(format!(
                "Unsupported photo format: {}",
                p.display()
            )));
        }
    }
    Ok(out)
}

fn parse_set_expr(expr: &str) -> Result<(&str, String, bool), CliError> {
    if let Some((k, v)) = expr.split_once("+=") {
        Ok((k.trim(), v.trim().to_owned(), true))
    } else if let Some((k, v)) = expr.split_once("-=") {
        let trimmed = v.trim();
        let neg = if let Some(pos) = trimmed.strip_prefix('-') {
            pos.to_owned()
        } else {
            format!("-{trimmed}")
        };
        Ok((k.trim(), neg, true))
    } else if let Some((k, v)) = expr.split_once('=') {
        let v_trim = v.trim();
        if let Some(rest) = v_trim.strip_prefix("+=") {
            Ok((k.trim(), rest.trim().to_owned(), true))
        } else {
            Ok((k.trim(), v_trim.to_owned(), false))
        }
    } else {
        Err(CliError::validation(format!(
            "Invalid --set expression '{expr}' (expected `key=value`, `key+=delta`, or `key-=delta`)"
        )))
    }
}

fn parse_xy_pair(raw: &str) -> Result<[f32; 2], CliError> {
    let (xs, ys) = raw
        .split_once(',')
        .ok_or_else(|| CliError::validation(format!("Expected `x,y` coordinates, got '{raw}'")))?;
    let x: f32 = xs
        .trim()
        .parse()
        .map_err(|_| CliError::validation(format!("Invalid x coordinate '{xs}'")))?;
    let y: f32 = ys
        .trim()
        .parse()
        .map_err(|_| CliError::validation(format!("Invalid y coordinate '{ys}'")))?;
    if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        return Err(CliError::validation(format!(
            "Coordinates must be normalized in 0.0..=1.0 (got {x},{y})"
        )));
    }
    Ok([x, y])
}

fn parse_color_slice_pick(raw: &str) -> Result<(usize, [f32; 2]), CliError> {
    let (idx_str, xy_str) = raw.split_once(':').ok_or_else(|| {
        CliError::validation(format!(
            "Invalid --pick-color-slice '{raw}' (expected `idx:x,y`, e.g. `0:0.5,0.4`)"
        ))
    })?;
    let idx: usize = idx_str.trim().parse().map_err(|_| {
        CliError::validation(format!(
            "Invalid color slice index '{idx_str}' (expected 0..={})",
            ADVANCED_COLOR_SLICE_COUNT - 1
        ))
    })?;
    if idx >= ADVANCED_COLOR_SLICE_COUNT {
        return Err(CliError::validation(format!(
            "Color slice index {idx} out of range (expected 0..={})",
            ADVANCED_COLOR_SLICE_COUNT - 1
        )));
    }
    let xy = parse_xy_pair(xy_str)?;
    Ok((idx, xy))
}

fn sample_patch_3x3(img: &rune_editor_decode::LinearImage, nx: f32, ny: f32) -> [f32; 3] {
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

fn load_preset_or_file(id_or_path: &str) -> Result<DevelopRecipe, CliError> {
    if let Ok(r) = resolve_builtin_preset(id_or_path) {
        return Ok(r);
    }
    let path = Path::new(id_or_path);
    if path.exists() {
        let text = std::fs::read_to_string(path).map_err(|e| CliError::io(e.to_string()))?;
        let meta = PhotoMeta::from_sidecar_json(&text)?;
        return Ok(meta.recipe);
    }
    Err(CliError::from(rune_editor_core::CoreError::UnknownPreset(
        id_or_path.to_owned(),
    )))
}

fn read_arg_or_stdin(raw: &str) -> Result<String, CliError> {
    if raw.trim() == "-" {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| CliError::io(e.to_string()))?;
        Ok(buf)
    } else {
        Ok(raw.to_owned())
    }
}

fn parse_export_format(raw: &str, quality: u8) -> Result<(ExportFormat, &'static str), CliError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Ok((ExportFormat::Jpeg { quality }, "jpg")),
        "png" | "png8" => Ok((ExportFormat::Png8, "png")),
        "png16" => Ok((ExportFormat::Png16, "png")),
        "tif" | "tiff" | "tiff16" => Ok((ExportFormat::Tiff16, "tif")),
        "webp" => Ok((ExportFormat::WebP, "webp")),
        other => Err(CliError::validation(format!(
            "Unsupported export format '{other}'. Valid: jpeg, png8, png16, tiff16, webp"
        ))),
    }
}

fn infer_format_from_ext(path: &Path, quality: u8) -> ExportFormat {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => ExportFormat::Png8,
        Some("webp") => ExportFormat::WebP,
        Some("tif" | "tiff") => ExportFormat::Tiff16,
        _ => ExportFormat::Jpeg { quality },
    }
}

fn render_dest_path(
    src: &Path,
    out_dir: &Path,
    template: &str,
    ext: &str,
) -> Result<PathBuf, CliError> {
    use std::path::Component;

    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "photo".to_owned());
    let fname = template.replace("{stem}", &stem).replace("{ext}", ext);
    if fname.trim().is_empty() || fname.contains('\0') || fname.contains('\\') {
        return Err(CliError::validation(format!(
            "Invalid export filename template '{template}'"
        )));
    }
    let rel = Path::new(&fname);
    let mut clean_rel = PathBuf::new();
    for comp in rel.components() {
        match comp {
            Component::Normal(seg) => {
                let s = seg.to_string_lossy();
                if s.starts_with('.') {
                    return Err(CliError::validation(format!(
                        "Hidden path segment '{s}' is not allowed in export filename template '{template}'"
                    )));
                }
                clean_rel.push(seg);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(CliError::validation(format!(
                    "Export filename template '{template}' must stay within output directory (path traversal rejected)"
                )));
            }
        }
    }
    if clean_rel.as_os_str().is_empty() {
        return Err(CliError::validation(format!(
            "Export filename template '{template}' resolved to an empty path"
        )));
    }
    Ok(out_dir.join(clean_rel))
}

fn sets_touch_geometry(sets: &[String]) -> bool {
    sets.iter().any(|expr| {
        let key = expr
            .split_once("+=")
            .or_else(|| expr.split_once("-="))
            .or_else(|| expr.split_once('='))
            .map_or("", |(k, _)| k.trim());
        canonical_param_key(key).starts_with("geometry.")
    })
}

fn check_duplicate_export_paths(pairs: &[(PathBuf, PathBuf)]) -> Result<(), CliError> {
    let mut seen: HashMap<String, &Path> = HashMap::with_capacity(pairs.len());
    for (src, dest) in pairs {
        let parent = dest.parent().unwrap_or_else(|| Path::new("."));
        let norm_parent = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
        let normalized = match dest.file_name() {
            Some(f) => norm_parent.join(f),
            None => norm_parent,
        };
        let mut key = normalized.to_string_lossy().into_owned();
        if cfg!(any(target_os = "macos", target_os = "windows")) {
            key.make_ascii_lowercase();
        }
        if let Some(prev) = seen.insert(key, src.as_path()) {
            let mut err = CliError::validation(format!(
                "Duplicate export output path '{}' mapped from both '{}' and '{}'.",
                dest.display(),
                prev.display(),
                src.display(),
            ));
            err.code = "DUPLICATE_OUTPUT_PATH";
            err.hint = Some(
                "Disambiguate outputs with a distinct `--naming` template or filter inputs so stems do not collide."
                    .to_owned(),
            );
            return Err(err);
        }
    }
    Ok(())
}

fn guard_not_source(src: &Path, dest: &Path) -> Result<(), CliError> {
    decode_guard_not_source(src, dest).map_err(|e| CliError::validation(e.to_string()))
}

fn parse_color_tag(raw: &str) -> Result<ColorTag, CliError> {
    ColorTag::from_cli_str(raw).ok_or_else(|| {
        CliError::validation(format!(
            "Unknown color tag '{raw}'. Valid: none, red, orange, yellow, green, blue, purple, pink"
        ))
    })
}

fn build_layers_summary(layers: &[AdjustmentLayer]) -> Vec<Value> {
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

fn resolve_layer_index(layers: &[AdjustmentLayer], selector: &str) -> Result<usize, CliError> {
    let sel = selector.trim();
    if let Ok(idx) = sel.parse::<usize>()
        && idx < layers.len()
    {
        return Ok(idx);
    }
    if let Some(pos) = layers.iter().position(|l| l.id == sel) {
        return Ok(pos);
    }
    Err(CliError::validation(format!(
        "Layer '{selector}' not found (photo has {} layer(s))",
        layers.len()
    )))
}

fn resolve_heal_index(spots: &[HealSpot], selector: &str) -> Result<usize, CliError> {
    let sel = selector.trim();
    if let Ok(idx) = sel.parse::<usize>()
        && idx < spots.len()
    {
        return Ok(idx);
    }
    if let Some(pos) = spots.iter().position(|s| s.id == sel) {
        return Ok(pos);
    }
    Err(CliError::validation(format!(
        "Heal spot '{selector}' not found (photo has {} spot(s))",
        spots.len()
    )))
}

fn next_layer_id(layers: &[AdjustmentLayer]) -> String {
    let mut n = layers.len() + 1;
    loop {
        let candidate = format!("layer-{n}");
        if !layers.iter().any(|l| l.id == candidate) {
            return candidate;
        }
        n += 1;
    }
}

fn next_heal_id(spots: &[HealSpot]) -> String {
    let mut n = spots.len() + 1;
    loop {
        let candidate = format!("heal-{n}");
        if !spots.iter().any(|s| s.id == candidate) {
            return candidate;
        }
        n += 1;
    }
}

fn apply_local_set_exprs(adj: &mut LocalAdjustments, exprs: &[String]) -> Result<(), CliError> {
    for expr in exprs {
        let (key, val_str, delta) = parse_set_expr(expr)?;
        let v: f32 = val_str.parse().map_err(|_| {
            CliError::validation(format!(
                "Invalid numeric value '{val_str}' for local adjustment '{key}'"
            ))
        })?;
        if !v.is_finite() {
            return Err(CliError::validation(format!(
                "Local adjustment '{key}' value must be finite"
            )));
        }
        let target = if delta { adj.get_by_name(key)? + v } else { v };
        adj.set_by_name(key, target)?;
    }
    Ok(())
}

fn parse_radius_pair(raw: &str) -> Result<[f32; 2], CliError> {
    if let Some((rx_s, ry_s)) = raw.split_once(',') {
        let rx: f32 = rx_s
            .trim()
            .parse()
            .map_err(|_| CliError::validation(format!("Invalid rx in --radius '{raw}'")))?;
        let ry: f32 = ry_s
            .trim()
            .parse()
            .map_err(|_| CliError::validation(format!("Invalid ry in --radius '{raw}'")))?;
        if !rx.is_finite()
            || !ry.is_finite()
            || !(0.01..=1.5).contains(&rx)
            || !(0.01..=1.5).contains(&ry)
        {
            return Err(CliError::validation(format!(
                "--radius components must be within 0.01..=1.5 (got {rx},{ry})"
            )));
        }
        Ok([rx, ry])
    } else {
        let r: f32 = raw
            .trim()
            .parse()
            .map_err(|_| CliError::validation(format!("Invalid --radius '{raw}'")))?;
        if !r.is_finite() || !(0.01..=1.5).contains(&r) {
            return Err(CliError::validation(format!(
                "--radius must be within 0.01..=1.5 (got {r})"
            )));
        }
        Ok([r, r])
    }
}

fn format_bytes_human(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    const KIB: f64 = 1024.0;
    if bytes as f64 >= MIB {
        format!("{:.1} MB", bytes as f64 / MIB)
    } else if bytes as f64 >= KIB {
        format!("{:.1} KB", bytes as f64 / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn model_install_state_str(state: ModelInstallState) -> &'static str {
    match state {
        ModelInstallState::NotInstalled => "not_installed",
        ModelInstallState::Partial { .. } => "partial",
        ModelInstallState::Installed { .. } => "installed",
    }
}

fn model_status_to_json(status: &rune_editor_core::ModelStatus) -> Value {
    json!({
        "id": status.id.id(),
        "display_name": status.display_name,
        "aliases": status.id.aliases(),
        "engine": status.engine,
        "hf_repo": status.hf_repo,
        "state": model_install_state_str(status.state),
        "installed": status.state.is_installed(),
        "installed_bytes": status.state.bytes_present(),
        "approx_bytes": status.approx_bytes,
        "license": status.license,
        "license_url": status.license_url,
        "requires_license_acceptance": status.requires_license_acceptance,
        "license_accepted": status.license_accepted,
        "install_dir": status.install_dir,
    })
}

fn parse_cli_model_id(raw: &str) -> Result<ModelId, CliError> {
    ModelId::from_str_loose(raw).ok_or_else(|| {
        CliError::validation(format!(
            "Unknown model '{raw}'. Valid: sam2.1-hiera-tiny (sam2.1, sam2), sam3-tracker-q4f16 (sam3), sam3-tracker-fp16 (sam3-fp16)"
        ))
    })
}

fn parse_cli_engine(raw: Option<&str>) -> Result<Option<SegmentationEngine>, CliError> {
    let Some(s) = raw else {
        return Ok(None);
    };
    let eng = SegmentationEngine::from_str_loose(s).ok_or_else(|| {
        CliError::validation(format!(
            "Unknown segmentation --engine '{s}'. Valid: auto, apple-vision, sam2.1, sam3, heuristic"
        ))
    })?;
    Ok(Some(eng))
}

fn parse_prompt_point(raw: &str) -> Result<PromptPoint, CliError> {
    let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
    if parts.len() != 2 && parts.len() != 3 {
        return Err(CliError::validation(format!(
            "Invalid --point '{raw}'. Expected `x,y` or `x,y,+` / `x,y,-` in 0.0..=1.0"
        )));
    }
    let x: f32 = parts[0]
        .parse()
        .map_err(|_| CliError::validation(format!("Invalid X coordinate in --point '{raw}'")))?;
    let y: f32 = parts[1]
        .parse()
        .map_err(|_| CliError::validation(format!("Invalid Y coordinate in --point '{raw}'")))?;
    if !x.is_finite() || !y.is_finite() || !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        return Err(CliError::validation(format!(
            "--point coordinates must be finite and within 0.0..=1.0 (got '{raw}')"
        )));
    }
    let positive = if parts.len() == 3 {
        match parts[2].to_ascii_lowercase().as_str() {
            "+" | "pos" | "positive" | "1" | "fg" | "true" => true,
            "-" | "neg" | "negative" | "0" | "bg" | "false" => false,
            other => {
                return Err(CliError::validation(format!(
                    "Invalid polarity '{other}' in --point '{raw}'. Expected `+` or `-`"
                )));
            }
        }
    } else {
        true
    };
    Ok(PromptPoint::new(x, y, positive))
}

fn parse_prompt_box(raw: &str) -> Result<PromptBox, CliError> {
    let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
    if parts.len() != 4 {
        return Err(CliError::validation(format!(
            "Invalid --box '{raw}'. Expected `x0,y0,x1,y1` in 0.0..=1.0"
        )));
    }
    let mut vals = [0.0_f32; 4];
    for (i, part) in parts.iter().enumerate() {
        let v: f32 = part
            .parse()
            .map_err(|_| CliError::validation(format!("Invalid number in --box '{raw}'")))?;
        if !v.is_finite() || !(0.0..=1.0).contains(&v) {
            return Err(CliError::validation(format!(
                "--box coordinates must be finite and within 0.0..=1.0 (got '{raw}')"
            )));
        }
        vals[i] = v;
    }
    let bbox = PromptBox::new(vals[0], vals[1], vals[2], vals[3]).sanitized();
    if !bbox.is_valid() {
        return Err(CliError::validation(format!(
            "--box must enclose a non-zero area (x0 != x1 and y0 != y1; got '{raw}')"
        )));
    }
    Ok(bbox)
}

fn attach_smart_telemetry(data: &mut Value, smart: &SmartMaskResult) {
    if let Some(obj) = data.as_object_mut() {
        obj.insert("engine_used".to_owned(), json!(smart.engine_used));
        obj.insert("backend_label".to_owned(), json!(smart.backend_label));
        obj.insert("elapsed_ms".to_owned(), json!(smart.elapsed_ms));
        obj.insert("instance_count".to_owned(), json!(smart.instance_count));
        obj.insert("iou_score".to_owned(), json!(smart.iou_score));
        obj.insert("fallback_note".to_owned(), json!(smart.fallback_note));
    }
}

fn cmd_model(sub: ModelCmd, opts: &RuntimeOpts) -> Result<u8, CliError> {
    match sub {
        ModelCmd::List => {
            let summary = active_engine_summary();
            let statuses = list_model_statuses();
            let builtin_apple = json!({
                "id": "apple-vision",
                "display_name": "Apple Vision ANE (Built-in)",
                "aliases": ["apple-vision", "vision", "apple"],
                "engine": SegmentationEngine::AppleVision,
                "available": summary.apple_vision_available,
                "backend_label": summary.apple_vision_label,
                "state": if summary.apple_vision_available { "built_in" } else { "unavailable" },
                "installed": summary.apple_vision_available,
                "installed_bytes": 0,
                "approx_bytes": 0,
                "license": "macOS System Framework",
                "requires_license_acceptance": false,
                "license_accepted": true,
            });
            let models_json: Vec<Value> = statuses.iter().map(model_status_to_json).collect();

            let mut lines = Vec::new();
            lines.push(format!(
                "Active engine: {} (Subject: {}, Interactive: {})",
                summary.configured_engine.display_name(),
                summary.effective_auto_engine.display_name(),
                summary.effective_interactive_engine.display_name(),
            ));
            lines.push(format!(
                "  apple-vision           {} [{}] (0 B, built-in)",
                summary.apple_vision_label,
                if summary.apple_vision_available {
                    "built-in"
                } else {
                    "unavailable"
                }
            ));
            for st in &statuses {
                let state_str = match st.state {
                    ModelInstallState::Installed { total_bytes } => {
                        format!("installed, {}", format_bytes_human(total_bytes))
                    }
                    ModelInstallState::Partial { bytes_present } => {
                        format!("partial, {}", format_bytes_human(bytes_present))
                    }
                    ModelInstallState::NotInstalled => {
                        format!("not installed, ~{}", format_bytes_human(st.approx_bytes))
                    }
                };
                let lic = if st.requires_license_acceptance {
                    if st.license_accepted {
                        format!("{} (accepted)", st.license)
                    } else {
                        format!("{} (--accept-license required)", st.license)
                    }
                } else {
                    st.license.clone()
                };
                lines.push(format!(
                    "  {:22} {} [{}] | aliases: {} | {}",
                    st.id.id(),
                    st.display_name,
                    state_str,
                    st.id.aliases().join(", "),
                    lic,
                ));
            }

            let data = json!({
                "active_engine": summary.configured_engine,
                "effective_auto_engine": summary.effective_auto_engine,
                "effective_interactive_engine": summary.effective_interactive_engine,
                "models_dir": summary.models_root,
                "builtin_apple_vision": builtin_apple,
                "models": models_json,
            });
            emit_ok(opts, "model.list", data, &lines.join("\n"));
            Ok(exit_code::OK)
        }
        ModelCmd::Status => {
            let summary = active_engine_summary();
            let statuses = list_model_statuses();
            let installed_total_bytes: u64 = statuses
                .iter()
                .map(|s| s.state.bytes_present())
                .fold(0u64, u64::saturating_add);
            let installed_names: Vec<&str> =
                summary.installed_models.iter().map(|id| id.id()).collect();

            let human = format!(
                "Active engine: {}\n  Effective Subject engine:     {}\n  Effective Interactive engine: {}\n  Apple Vision:                 {} ({})\n  Models directory:             {}\n  Installed models:             {}\n  Installed disk usage:         {}",
                summary.configured_engine.display_name(),
                summary.effective_auto_engine.display_name(),
                summary.effective_interactive_engine.display_name(),
                if summary.apple_vision_available {
                    "available"
                } else {
                    "unavailable"
                },
                summary.apple_vision_label,
                summary.models_root.display(),
                if installed_names.is_empty() {
                    "none".to_owned()
                } else {
                    installed_names.join(", ")
                },
                format_bytes_human(installed_total_bytes),
            );
            let data = json!({
                "active_engine": summary.configured_engine,
                "configured_engine": summary.configured_engine,
                "effective_auto_engine": summary.effective_auto_engine,
                "effective_interactive_engine": summary.effective_interactive_engine,
                "apple_vision_available": summary.apple_vision_available,
                "apple_vision_label": summary.apple_vision_label,
                "installed_models": summary.installed_models,
                "preferred_sam3_variant": summary.preferred_sam3_variant,
                "models_dir": summary.models_root,
                "installed_total_bytes": installed_total_bytes,
            });
            emit_ok(opts, "model.status", data, &human);
            Ok(exit_code::OK)
        }
        ModelCmd::Setup {
            model,
            accept_license,
            from_dir,
        } => {
            let model_id = parse_cli_model_id(&model)?;
            let spec = model_id.spec();
            if spec.requires_license_acceptance && !accept_license {
                return Err(rune_editor_decode::ModelError::LicenseRequired {
                    model_id,
                    license: spec.license.to_owned(),
                    license_url: spec.license_url.to_owned(),
                }
                .into());
            }

            if opts.dry_run {
                if let Some(dir) = &from_dir
                    && !dir.is_dir()
                {
                    return Err(CliError::not_found(format!(
                        "Local model directory not found: {}",
                        dir.display()
                    )));
                }
                let config = load_ai_model_config().unwrap_or_default();
                let st = inspect_model_status(model_id, &config);
                let data = json!({
                    "model_id": model_id.id(),
                    "source": if from_dir.is_some() { "local_dir" } else { "huggingface" },
                    "from_dir": from_dir,
                    "would_install_dir": st.install_dir,
                    "approx_bytes": spec.approx_bytes,
                    "license": spec.license,
                    "license_accepted": accept_license || !spec.requires_license_acceptance,
                });
                let human = format!(
                    "[dry-run] Would install {} ({}, ~{}) into {}",
                    spec.display_name,
                    model_id.id(),
                    format_bytes_human(spec.approx_bytes),
                    st.install_dir.display(),
                );
                emit_ok(opts, "model.setup", data, &human);
                return Ok(exit_code::OK);
            }

            let source = if from_dir.is_some() {
                "local_dir"
            } else {
                "huggingface"
            };
            let status = if let Some(dir) = from_dir.as_deref() {
                setup_model_from_local_dir(model_id, dir, accept_license)?
            } else {
                let last_pct = std::cell::Cell::new(usize::MAX);
                let last_file = std::cell::Cell::new(usize::MAX);
                download_model(model_id, accept_license, None, |prog| {
                    if opts.quiet {
                        return;
                    }
                    let pct = ((prog.overall_fraction.clamp(0.0, 1.0)) * 100.0).round() as usize;
                    let prev_pct = last_pct.get();
                    let prev_file = last_file.get();
                    if prev_file != prog.file_index
                        || prev_pct == usize::MAX
                        || pct >= prev_pct.saturating_add(5)
                        || pct >= 100
                    {
                        last_pct.set(pct);
                        last_file.set(prog.file_index);
                        eprintln!(
                            "[{:3}%] ({}/{}) {} — {}",
                            pct,
                            prog.file_index + 1,
                            prog.total_files,
                            prog.file_name,
                            prog.status,
                        );
                    }
                })?
            };

            let mut data = model_status_to_json(&status);
            if let Some(obj) = data.as_object_mut() {
                obj.insert("source".to_owned(), json!(source));
            }
            let human = format!(
                "Installed {} ({}, {}) into {}",
                status.display_name,
                status.id.id(),
                format_bytes_human(status.state.bytes_present()),
                status.install_dir.display(),
            );
            emit_ok(opts, "model.setup", data, &human);
            Ok(exit_code::OK)
        }
        ModelCmd::Use { engine } => {
            let eng = SegmentationEngine::from_str_loose(&engine).ok_or_else(|| {
                CliError::validation(format!(
                    "Unknown segmentation engine '{engine}'. Valid: auto, apple-vision, sam2.1, sam3, heuristic"
                ))
            })?;
            let config = if opts.dry_run {
                let mut preview = load_ai_model_config()?;
                preview.active_engine = eng;
                if let Some(m) = ModelId::from_str_loose(&engine)
                    && m.is_sam3()
                {
                    preview.preferred_sam3_variant = m;
                }
                preview
            } else {
                update_ai_model_config(|cfg| {
                    cfg.active_engine = eng;
                    if let Some(m) = ModelId::from_str_loose(&engine)
                        && m.is_sam3()
                    {
                        cfg.preferred_sam3_variant = m;
                    }
                })?
            };
            let summary = active_engine_summary();
            let active_engine = if opts.dry_run {
                eng
            } else {
                summary.configured_engine
            };
            let data = json!({
                "active_engine": active_engine,
                "effective_auto_engine": summary.effective_auto_engine,
                "effective_interactive_engine": summary.effective_interactive_engine,
                "preferred_sam3_variant": config.preferred_sam3_variant,
                "models_dir": summary.models_root,
            });
            let human = format!(
                "{} active segmentation engine to {} (Subject: {}, Interactive: {})",
                if opts.dry_run {
                    "[dry-run] Would set"
                } else {
                    "Set"
                },
                active_engine.display_name(),
                summary.effective_auto_engine.display_name(),
                summary.effective_interactive_engine.display_name(),
            );
            emit_ok(opts, "model.use", data, &human);
            Ok(exit_code::OK)
        }
        ModelCmd::Remove { model } => {
            let model_id = parse_cli_model_id(&model)?;
            let config = load_ai_model_config().unwrap_or_default();
            let before_status = inspect_model_status(model_id, &config);
            let freed_bytes = before_status.state.bytes_present();
            if !opts.dry_run {
                remove_model(model_id)?;
            }
            let after_status = inspect_model_status(model_id, &config);
            let data = json!({
                "model_id": model_id.id(),
                "removed": !opts.dry_run,
                "freed_bytes": freed_bytes,
                "status": model_status_to_json(&after_status),
            });
            let human = format!(
                "{} model '{}' (freed {})",
                if opts.dry_run {
                    "[dry-run] Would remove"
                } else {
                    "Removed"
                },
                model_id.id(),
                format_bytes_human(freed_bytes),
            );
            emit_ok(opts, "model.remove", data, &human);
            Ok(exit_code::OK)
        }
    }
}

fn cmd_layer(sub: LayerCmd, opts: &RuntimeOpts) -> Result<u8, CliError> {
    match sub {
        LayerCmd::List { photo } => {
            ensure_exists(&photo)?;
            let meta = PhotoMeta::load_sidecar(&photo)?.unwrap_or_default();
            let layers = build_layers_summary(&meta.recipe.layers);
            let data = json!({
                "path": photo,
                "layer_count": meta.recipe.layers.len(),
                "layers": layers,
            });
            let human = format!(
                "{}: {} adjustment layer(s)",
                photo.display(),
                meta.recipe.layers.len()
            );
            emit_ok(opts, "layer.list", data, &human);
            Ok(exit_code::OK)
        }
        LayerCmd::Add {
            photo,
            name,
            kind,
            opacity,
            sets,
            engine,
            points,
            bbox,
            invert,
        } => {
            ensure_exists(&photo)?;
            if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) {
                return Err(CliError::validation(format!(
                    "--opacity must be within 0.0..=1.0 (got {opacity})"
                )));
            }
            let engine_override = parse_cli_engine(engine.as_deref())?;
            let mut meta = PhotoMeta::load_sidecar(&photo)?.unwrap_or_default();
            if meta.recipe.layers.len() >= MAX_ADJUSTMENT_LAYERS {
                return Err(CliError::validation(format!(
                    "Maximum {MAX_ADJUSTMENT_LAYERS} adjustment layers reached"
                )));
            }
            let before = meta.recipe.clone();
            let id = next_layer_id(&meta.recipe.layers);
            let norm_kind = kind.trim().to_ascii_lowercase().replace('_', "-");
            let is_ai_select = matches!(
                norm_kind.as_str(),
                "ai-select" | "select" | "smart" | "smart-select"
            );
            if !is_ai_select && (!points.is_empty() || bbox.is_some()) {
                return Err(CliError::validation(
                    "`--point` and `--box` are only valid with `--kind ai-select`.",
                ));
            }
            let mut smart_telemetry: Option<SmartMaskResult> = None;
            let mut layer = match norm_kind.as_str() {
                "empty" => AdjustmentLayer::new(&id, &name),
                "filled" => AdjustmentLayer::new_filled(&id, &name),
                "linear" => AdjustmentLayer::new_with_mask(
                    &id,
                    &name,
                    MaskShape::LinearGradient {
                        start: [0.5, 0.0],
                        end: [0.5, 0.55],
                        feather: 1.0,
                    },
                ),
                "radial" => AdjustmentLayer::new_with_mask(
                    &id,
                    &name,
                    MaskShape::RadialGradient {
                        center: [0.5, 0.5],
                        radius: [0.35, 0.35],
                        angle_deg: 0.0,
                        feather: 0.4,
                    },
                ),
                "luma" => AdjustmentLayer::new_with_mask(
                    &id,
                    &name,
                    MaskShape::LumaRange {
                        low: 0.5,
                        high: 1.0,
                        low_falloff: 0.15,
                        high_falloff: 0.1,
                    },
                ),
                "ai-subject" | "subject" => {
                    let proxy = load_file_proxy(&photo, Some(1024))?;
                    let prompt = if invert {
                        SmartMaskPrompt::AutoBackground
                    } else {
                        SmartMaskPrompt::AutoSubject
                    };
                    let res = generate_smart_mask(&proxy, &prompt, engine_override)?;
                    let shape = res.mask.clone();
                    smart_telemetry = Some(res);
                    AdjustmentLayer::new_with_mask(&id, &name, shape)
                }
                "ai-background" | "background" | "ai-bg" => {
                    let proxy = load_file_proxy(&photo, Some(1024))?;
                    let prompt = if invert {
                        SmartMaskPrompt::AutoSubject
                    } else {
                        SmartMaskPrompt::AutoBackground
                    };
                    let res = generate_smart_mask(&proxy, &prompt, engine_override)?;
                    let shape = res.mask.clone();
                    smart_telemetry = Some(res);
                    AdjustmentLayer::new_with_mask(&id, &name, shape)
                }
                "ai-select" | "select" | "smart" | "smart-select" => {
                    if points.len() > MAX_PROMPT_POINTS {
                        return Err(CliError::validation(format!(
                            "Too many --point arguments ({} > max {MAX_PROMPT_POINTS})",
                            points.len()
                        )));
                    }
                    let mut parsed_points = Vec::with_capacity(points.len());
                    for p in &points {
                        parsed_points.push(parse_prompt_point(p)?);
                    }
                    let parsed_box = match bbox.as_deref() {
                        Some(b) => Some(parse_prompt_box(b)?),
                        None => None,
                    };
                    if parsed_points.is_empty() && parsed_box.is_none() {
                        return Err(CliError::validation(
                            "`--kind ai-select` requires at least one `--point x,y[,+|-]` or `--box x0,y0,x1,y1`.",
                        ));
                    }
                    let proxy = load_file_proxy(&photo, Some(1024))?;
                    let prompt = SmartMaskPrompt::Interactive {
                        points: parsed_points,
                        bbox: parsed_box,
                        invert,
                    };
                    let res = generate_smart_mask(&proxy, &prompt, engine_override)?;
                    let shape = res.mask.clone();
                    smart_telemetry = Some(res);
                    AdjustmentLayer::new_with_mask(&id, &name, shape)
                }
                other => {
                    return Err(CliError::validation(format!(
                        "Unknown layer --kind '{other}'. Valid: empty, filled, linear, radial, luma, ai-subject, ai-background, ai-select"
                    )));
                }
            };
            if !matches!(
                norm_kind.as_str(),
                "ai-subject"
                    | "subject"
                    | "ai-background"
                    | "background"
                    | "ai-bg"
                    | "ai-select"
                    | "select"
                    | "smart"
                    | "smart-select"
            ) && invert
            {
                layer.inverted = true;
            }
            layer.opacity = opacity;
            apply_local_set_exprs(&mut layer.adjustments, &sets)?;
            layer = layer.sanitized();
            meta.recipe.layers.push(layer);
            meta.recipe.preset_name = None;
            let idx = meta.recipe.layers.len() - 1;
            let diffs = recipe_field_diffs(&before, &meta.recipe);
            if !opts.dry_run {
                meta.push_history(
                    EditActor::Agent {
                        name: "layer".to_owned(),
                        job_id: None,
                    },
                    format!("rd layer add '{}' ({kind})", name),
                    diffs,
                    Some(before),
                );
                meta.save_sidecar_in_place(&photo)?;
            }
            let mut data = json!({
                "path": photo,
                "index": idx,
                "id": id,
                "layer_count": meta.recipe.layers.len(),
                "layers": build_layers_summary(&meta.recipe.layers),
            });
            let engine_note = if let Some(ref smart) = smart_telemetry {
                attach_smart_telemetry(&mut data, smart);
                format!(" via {} ({}ms)", smart.backend_label, smart.elapsed_ms)
            } else {
                String::new()
            };
            let human = format!(
                "Added layer #{idx} '{name}' ({kind}){engine_note} on {}",
                photo.display()
            );
            emit_ok(opts, "layer.add", data, &human);
            Ok(exit_code::OK)
        }
        LayerCmd::Update {
            photo,
            index,
            name,
            opacity,
            enabled,
            inverted,
            sets,
        } => {
            ensure_exists(&photo)?;
            if let Some(op) = opacity
                && (!op.is_finite() || !(0.0..=1.0).contains(&op))
            {
                return Err(CliError::validation(format!(
                    "--opacity must be within 0.0..=1.0 (got {op})"
                )));
            }
            let mut meta = PhotoMeta::load_sidecar(&photo)?.unwrap_or_default();
            let idx = resolve_layer_index(&meta.recipe.layers, &index)?;
            let before = meta.recipe.clone();
            {
                let layer = &mut meta.recipe.layers[idx];
                if let Some(n) = name {
                    layer.name = n;
                }
                if let Some(op) = opacity {
                    layer.opacity = op;
                }
                if let Some(en) = enabled {
                    layer.enabled = en;
                }
                if let Some(inv) = inverted {
                    layer.inverted = inv;
                }
                apply_local_set_exprs(&mut layer.adjustments, &sets)?;
                *layer = layer.sanitized();
            }
            meta.recipe.preset_name = None;
            let diffs = recipe_field_diffs(&before, &meta.recipe);
            if !opts.dry_run && !diffs.is_empty() {
                meta.push_history(
                    EditActor::Agent {
                        name: "layer".to_owned(),
                        job_id: None,
                    },
                    format!("rd layer update #{idx}"),
                    diffs,
                    Some(before),
                );
                meta.save_sidecar_in_place(&photo)?;
            }
            let data = json!({
                "path": photo,
                "index": idx,
                "layer_count": meta.recipe.layers.len(),
                "layers": build_layers_summary(&meta.recipe.layers),
            });
            let human = format!("Updated layer #{idx} on {}", photo.display());
            emit_ok(opts, "layer.update", data, &human);
            Ok(exit_code::OK)
        }
        LayerCmd::MaskAdd {
            photo,
            index,
            kind,
            op,
            start,
            end,
            center,
            radius,
            angle,
            feather,
            low,
            high,
            low_falloff,
            high_falloff,
            inverted,
            engine,
            points,
            bbox,
            invert,
        } => {
            ensure_exists(&photo)?;
            let combine_op = MaskCombineOp::from_cli_str(&op).ok_or_else(|| {
                CliError::validation(format!(
                    "Unknown --op '{op}'. Valid: add, subtract, intersect"
                ))
            })?;
            let engine_override = parse_cli_engine(engine.as_deref())?;
            let mut meta = PhotoMeta::load_sidecar(&photo)?.unwrap_or_default();
            let idx = resolve_layer_index(&meta.recipe.layers, &index)?;
            if meta.recipe.layers[idx].masks.len() >= MAX_MASKS_PER_LAYER {
                return Err(CliError::validation(format!(
                    "Layer #{idx} already has the maximum {MAX_MASKS_PER_LAYER} masks"
                )));
            }
            let norm_kind = kind.trim().to_ascii_lowercase().replace('_', "-");
            let is_ai_select = matches!(
                norm_kind.as_str(),
                "ai-select" | "select" | "smart" | "smart-select"
            );
            if !is_ai_select && (!points.is_empty() || bbox.is_some()) {
                return Err(CliError::validation(
                    "`--point` and `--box` are only valid with `--kind ai-select`.",
                ));
            }
            let mut smart_telemetry: Option<SmartMaskResult> = None;
            let shape = match norm_kind.as_str() {
                "filled" => MaskShape::Filled,
                "linear" => {
                    let s = match start.as_deref() {
                        Some(v) => parse_xy_pair(v)?,
                        None => [0.5, 0.0],
                    };
                    let e = match end.as_deref() {
                        Some(v) => parse_xy_pair(v)?,
                        None => [0.5, 0.55],
                    };
                    let f = feather.unwrap_or(1.0);
                    if !f.is_finite() || !(0.0..=1.0).contains(&f) {
                        return Err(CliError::validation(format!(
                            "--feather must be within 0.0..=1.0 (got {f})"
                        )));
                    }
                    MaskShape::LinearGradient {
                        start: s,
                        end: e,
                        feather: f,
                    }
                }
                "radial" => {
                    let c = match center.as_deref() {
                        Some(v) => parse_xy_pair(v)?,
                        None => [0.5, 0.5],
                    };
                    let r = match radius.as_deref() {
                        Some(v) => parse_radius_pair(v)?,
                        None => [0.35, 0.35],
                    };
                    let f = feather.unwrap_or(0.4);
                    if !f.is_finite() || !(0.0..=1.0).contains(&f) {
                        return Err(CliError::validation(format!(
                            "--feather must be within 0.0..=1.0 (got {f})"
                        )));
                    }
                    MaskShape::RadialGradient {
                        center: c,
                        radius: r,
                        angle_deg: angle,
                        feather: f,
                    }
                }
                "luma" => {
                    for (lbl, val) in [
                        ("--low", low),
                        ("--high", high),
                        ("--low-falloff", low_falloff),
                        ("--high-falloff", high_falloff),
                    ] {
                        if !val.is_finite() || !(0.0..=1.0).contains(&val) {
                            return Err(CliError::validation(format!(
                                "{lbl} must be within 0.0..=1.0 (got {val})"
                            )));
                        }
                    }
                    if low > high {
                        return Err(CliError::validation(format!(
                            "--low ({low}) must be <= --high ({high}) for luma mask"
                        )));
                    }
                    MaskShape::LumaRange {
                        low,
                        high,
                        low_falloff,
                        high_falloff,
                    }
                }
                "ai-subject" | "subject" => {
                    let proxy = load_file_proxy(&photo, Some(1024))?;
                    let prompt = if invert {
                        SmartMaskPrompt::AutoBackground
                    } else {
                        SmartMaskPrompt::AutoSubject
                    };
                    let res = generate_smart_mask(&proxy, &prompt, engine_override)?;
                    let mask = res.mask.clone();
                    smart_telemetry = Some(res);
                    mask
                }
                "ai-background" | "background" | "ai-bg" => {
                    let proxy = load_file_proxy(&photo, Some(1024))?;
                    let prompt = if invert {
                        SmartMaskPrompt::AutoSubject
                    } else {
                        SmartMaskPrompt::AutoBackground
                    };
                    let res = generate_smart_mask(&proxy, &prompt, engine_override)?;
                    let mask = res.mask.clone();
                    smart_telemetry = Some(res);
                    mask
                }
                "ai-select" | "select" | "smart" | "smart-select" => {
                    if points.len() > MAX_PROMPT_POINTS {
                        return Err(CliError::validation(format!(
                            "Too many --point arguments ({} > max {MAX_PROMPT_POINTS})",
                            points.len()
                        )));
                    }
                    let mut parsed_points = Vec::with_capacity(points.len());
                    for p in &points {
                        parsed_points.push(parse_prompt_point(p)?);
                    }
                    let parsed_box = match bbox.as_deref() {
                        Some(b) => Some(parse_prompt_box(b)?),
                        None => None,
                    };
                    if parsed_points.is_empty() && parsed_box.is_none() {
                        return Err(CliError::validation(
                            "`--kind ai-select` requires at least one `--point x,y[,+|-]` or `--box x0,y0,x1,y1`.",
                        ));
                    }
                    let proxy = load_file_proxy(&photo, Some(1024))?;
                    let prompt = SmartMaskPrompt::Interactive {
                        points: parsed_points,
                        bbox: parsed_box,
                        invert,
                    };
                    let res = generate_smart_mask(&proxy, &prompt, engine_override)?;
                    let mask = res.mask.clone();
                    smart_telemetry = Some(res);
                    mask
                }
                other => {
                    return Err(CliError::validation(format!(
                        "Unknown mask --kind '{other}'. Valid: linear, radial, luma, filled, ai-subject, ai-background, ai-select"
                    )));
                }
            };
            let before = meta.recipe.clone();
            let mut item = LayerMaskItem::new(shape);
            item.op = combine_op;
            item.inverted = inverted
                || (invert
                    && !matches!(
                        norm_kind.as_str(),
                        "ai-subject"
                            | "subject"
                            | "ai-background"
                            | "background"
                            | "ai-bg"
                            | "ai-select"
                            | "select"
                            | "smart"
                            | "smart-select"
                    ));
            meta.recipe.layers[idx].masks.push(item.sanitized());
            meta.recipe.preset_name = None;
            let diffs = recipe_field_diffs(&before, &meta.recipe);
            if !opts.dry_run {
                meta.push_history(
                    EditActor::Agent {
                        name: "layer".to_owned(),
                        job_id: None,
                    },
                    format!("rd layer mask-add #{idx} ({kind}, {op})"),
                    diffs,
                    Some(before),
                );
                meta.save_sidecar_in_place(&photo)?;
            }
            let mut data = json!({
                "path": photo,
                "index": idx,
                "mask_count": meta.recipe.layers[idx].masks.len(),
                "layers": build_layers_summary(&meta.recipe.layers),
            });
            let engine_note = if let Some(ref smart) = smart_telemetry {
                attach_smart_telemetry(&mut data, smart);
                format!(" via {} ({}ms)", smart.backend_label, smart.elapsed_ms)
            } else {
                String::new()
            };
            let human = format!(
                "Added {kind} mask ({op}){engine_note} to layer #{idx} on {}",
                photo.display()
            );
            emit_ok(opts, "layer.mask_add", data, &human);
            Ok(exit_code::OK)
        }
        LayerCmd::MaskClear { photo, index } => {
            ensure_exists(&photo)?;
            let mut meta = PhotoMeta::load_sidecar(&photo)?.unwrap_or_default();
            let idx = resolve_layer_index(&meta.recipe.layers, &index)?;
            let before = meta.recipe.clone();
            meta.recipe.layers[idx].masks.clear();
            meta.recipe.preset_name = None;
            let diffs = recipe_field_diffs(&before, &meta.recipe);
            if !opts.dry_run && !diffs.is_empty() {
                meta.push_history(
                    EditActor::Agent {
                        name: "layer".to_owned(),
                        job_id: None,
                    },
                    format!("rd layer mask-clear #{idx}"),
                    diffs,
                    Some(before),
                );
                meta.save_sidecar_in_place(&photo)?;
            }
            let data = json!({
                "path": photo,
                "index": idx,
                "mask_count": 0,
                "layers": build_layers_summary(&meta.recipe.layers),
            });
            let human = format!("Cleared masks on layer #{idx} of {}", photo.display());
            emit_ok(opts, "layer.mask_clear", data, &human);
            Ok(exit_code::OK)
        }
        LayerCmd::Remove { photo, index } => {
            ensure_exists(&photo)?;
            let mut meta = PhotoMeta::load_sidecar(&photo)?.unwrap_or_default();
            let idx = resolve_layer_index(&meta.recipe.layers, &index)?;
            let before = meta.recipe.clone();
            let removed = meta.recipe.layers.remove(idx);
            meta.recipe.preset_name = None;
            let diffs = recipe_field_diffs(&before, &meta.recipe);
            if !opts.dry_run {
                meta.push_history(
                    EditActor::Agent {
                        name: "layer".to_owned(),
                        job_id: None,
                    },
                    format!("rd layer remove '{}'", removed.name),
                    diffs,
                    Some(before),
                );
                meta.save_sidecar_in_place(&photo)?;
            }
            let data = json!({
                "path": photo,
                "removed_id": removed.id,
                "layer_count": meta.recipe.layers.len(),
                "layers": build_layers_summary(&meta.recipe.layers),
            });
            let human = format!(
                "Removed layer '{}' on {} ({} remaining)",
                removed.name,
                photo.display(),
                meta.recipe.layers.len()
            );
            emit_ok(opts, "layer.remove", data, &human);
            Ok(exit_code::OK)
        }
    }
}

fn cmd_heal(sub: HealCmd, opts: &RuntimeOpts) -> Result<u8, CliError> {
    match sub {
        HealCmd::List { photo } => {
            ensure_exists(&photo)?;
            let meta = PhotoMeta::load_sidecar(&photo)?.unwrap_or_default();
            let data = json!({
                "path": photo,
                "heal_spot_count": meta.recipe.heal_spots.len(),
                "heal_spots": meta.recipe.heal_spots,
            });
            let human = format!(
                "{}: {} heal/clone spot(s)",
                photo.display(),
                meta.recipe.heal_spots.len()
            );
            emit_ok(opts, "heal.list", data, &human);
            Ok(exit_code::OK)
        }
        HealCmd::Add {
            photo,
            target,
            source,
            radius,
            feather,
            opacity,
            mode,
        } => {
            ensure_exists(&photo)?;
            let target_xy = parse_xy_pair(&target)?;
            if !radius.is_finite() || !(0.002..=0.25).contains(&radius) {
                return Err(CliError::validation(format!(
                    "--radius must be within 0.002..=0.25 (got {radius})"
                )));
            }
            if !feather.is_finite() || !(0.0..=1.0).contains(&feather) {
                return Err(CliError::validation(format!(
                    "--feather must be within 0.0..=1.0 (got {feather})"
                )));
            }
            if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) {
                return Err(CliError::validation(format!(
                    "--opacity must be within 0.0..=1.0 (got {opacity})"
                )));
            }
            let heal_mode = HealMode::from_cli_str(&mode).ok_or_else(|| {
                CliError::validation(format!("Unknown --mode '{mode}'. Valid: heal, clone"))
            })?;
            let source_xy = match source.as_deref() {
                Some(s) => parse_xy_pair(s)?,
                None => HealSpot::auto_source_for_target(target_xy, radius),
            };
            let mut meta = PhotoMeta::load_sidecar(&photo)?.unwrap_or_default();
            if meta.recipe.heal_spots.len() >= MAX_HEAL_SPOTS {
                return Err(CliError::validation(format!(
                    "Maximum {MAX_HEAL_SPOTS} heal/clone spots reached"
                )));
            }
            let before = meta.recipe.clone();
            let id = next_heal_id(&meta.recipe.heal_spots);
            let spot = HealSpot {
                id: id.clone(),
                target: target_xy,
                source: source_xy,
                radius,
                feather,
                opacity,
                mode: heal_mode,
            }
            .sanitized();
            meta.recipe.heal_spots.push(spot.clone());
            meta.recipe.preset_name = None;
            let idx = meta.recipe.heal_spots.len() - 1;
            let diffs = recipe_field_diffs(&before, &meta.recipe);
            if !opts.dry_run {
                meta.push_history(
                    EditActor::Agent {
                        name: "heal".to_owned(),
                        job_id: None,
                    },
                    format!("rd heal add ({mode})"),
                    diffs,
                    Some(before),
                );
                meta.save_sidecar_in_place(&photo)?;
            }
            let data = json!({
                "path": photo,
                "index": idx,
                "spot": spot,
                "heal_spot_count": meta.recipe.heal_spots.len(),
            });
            let human = format!(
                "Added {} spot #{idx} ({id}) on {}",
                heal_mode.label(),
                photo.display()
            );
            emit_ok(opts, "heal.add", data, &human);
            Ok(exit_code::OK)
        }
        HealCmd::Remove { photo, index } => {
            ensure_exists(&photo)?;
            let mut meta = PhotoMeta::load_sidecar(&photo)?.unwrap_or_default();
            let idx = resolve_heal_index(&meta.recipe.heal_spots, &index)?;
            let before = meta.recipe.clone();
            let removed = meta.recipe.heal_spots.remove(idx);
            meta.recipe.preset_name = None;
            let diffs = recipe_field_diffs(&before, &meta.recipe);
            if !opts.dry_run {
                meta.push_history(
                    EditActor::Agent {
                        name: "heal".to_owned(),
                        job_id: None,
                    },
                    format!("rd heal remove '{}'", removed.id),
                    diffs,
                    Some(before),
                );
                meta.save_sidecar_in_place(&photo)?;
            }
            let data = json!({
                "path": photo,
                "removed_id": removed.id,
                "heal_spot_count": meta.recipe.heal_spots.len(),
            });
            let human = format!("Removed heal spot '{}' on {}", removed.id, photo.display());
            emit_ok(opts, "heal.remove", data, &human);
            Ok(exit_code::OK)
        }
        HealCmd::Clear { photo } => {
            ensure_exists(&photo)?;
            let mut meta = PhotoMeta::load_sidecar(&photo)?.unwrap_or_default();
            let before = meta.recipe.clone();
            let cleared = meta.recipe.heal_spots.len();
            meta.recipe.heal_spots.clear();
            meta.recipe.preset_name = None;
            let diffs = recipe_field_diffs(&before, &meta.recipe);
            if !opts.dry_run && !diffs.is_empty() {
                meta.push_history(
                    EditActor::Agent {
                        name: "heal".to_owned(),
                        job_id: None,
                    },
                    format!("rd heal clear ({cleared} spots)"),
                    diffs,
                    Some(before),
                );
                meta.save_sidecar_in_place(&photo)?;
            }
            let data = json!({
                "path": photo,
                "cleared": cleared,
                "heal_spot_count": 0,
            });
            let human = format!("Cleared {cleared} heal spot(s) on {}", photo.display());
            emit_ok(opts, "heal.clear", data, &human);
            Ok(exit_code::OK)
        }
    }
}

fn build_variants_summary(meta: &PhotoMeta) -> Vec<Value> {
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

fn build_naming_context(
    src: &Path,
    counter: usize,
    meta: &PhotoMeta,
    recipe_name: &str,
) -> NamingContext {
    let image_name = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "photo".to_owned());
    let exif = extract_exif_from_path(src);
    let (camera, lens, iso) = match exif {
        Some(ex) => (
            ex.camera_model.unwrap_or_default(),
            ex.lens_model.unwrap_or_default(),
            ex.iso.map(|i| format!("ISO{i}")).unwrap_or_default(),
        ),
        None => (String::new(), String::new(), String::new()),
    };
    NamingContext {
        image_name,
        counter,
        date: String::new(),
        camera,
        lens,
        iso,
        variant_index: meta.active_variant_index,
        recipe_name: recipe_name.to_owned(),
    }
}

fn parse_export_recipes_arg(
    raw: &str,
    base_opts: &ExportOptions,
    naming_template: Option<&str>,
) -> Result<Vec<ExportRecipeSpec>, CliError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CliError::validation(
            "--recipes cannot be empty (pass comma-separated slugs, JSON array, or path to JSON file)",
        ));
    }
    let mut specs: Vec<ExportRecipeSpec> = if trimmed.starts_with('[') {
        serde_json::from_str(trimmed)
            .map_err(|e| CliError::validation(format!("Invalid --recipes JSON: {e}")))?
    } else if Path::new(trimmed).exists() {
        let text = std::fs::read_to_string(trimmed).map_err(|e| CliError::io(e.to_string()))?;
        serde_json::from_str(&text)
            .map_err(|e| CliError::validation(format!("Invalid --recipes JSON file: {e}")))?
    } else {
        let mut builtins = builtin_export_recipes();
        for b in &mut builtins {
            b.enabled = false;
        }
        for tok in trimmed.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let norm = tok.to_ascii_lowercase().replace(['-', ' '], "_");
            match norm.as_str() {
                "all" => {
                    for b in &mut builtins {
                        b.enabled = true;
                    }
                }
                "full_res_jpeg" | "full" | "jpeg" | "jpg" => builtins[0].enabled = true,
                "web_2048px" | "web" | "web2048" => builtins[1].enabled = true,
                "archival_16_bit_tiff" | "archival" | "tiff" | "tiff16" => {
                    builtins[2].enabled = true;
                }
                other => {
                    return Err(CliError::validation(format!(
                        "Unknown export recipe '{other}'. Valid: all, full_res_jpeg, web_2048px, archival_16_bit_tiff (or JSON array/file)"
                    )));
                }
            }
        }
        builtins.into_iter().filter(|r| r.enabled).collect()
    };

    if specs.is_empty() || !specs.iter().any(|r| r.enabled) {
        return Err(CliError::validation(
            "No enabled export recipes specified in --recipes.",
        ));
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
        if !base_opts.respect_crop {
            r.options.respect_crop = false;
        }
    }
    Ok(specs)
}

fn run_multi_recipe_export(
    files: &[PathBuf],
    args: &ExportArgs,
    recipes_raw: &str,
    base_opts: &ExportOptions,
    opts: &RuntimeOpts,
) -> Result<u8, CliError> {
    let recipes =
        parse_export_recipes_arg(recipes_raw, base_opts, args.naming_template.as_deref())?;
    let start = Instant::now();

    // Pre-flight EVERY photo × enabled recipe path BEFORE writing any bytes to disk so
    // duplicate destinations, traversal attempts, source overwrites, and existing files fail fast.
    let mut all_pairs: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut contexts: Vec<(PhotoMeta, NamingContext)> = Vec::with_capacity(files.len());
    for (idx, src) in files.iter().enumerate() {
        let meta = PhotoMeta::load_sidecar(src)?.unwrap_or_default();
        let ctx = build_naming_context(src, idx + 1, &meta, "");
        let planned = plan_multi_recipe_paths(
            &args.out,
            &recipes,
            &ctx,
            Some(src.as_path()),
            args.overwrite,
        )?;
        for (_, dest) in planned {
            all_pairs.push((src.clone(), dest));
        }
        contexts.push((meta, ctx));
    }
    check_duplicate_export_paths(&all_pairs)?;

    let mut outputs = Vec::new();
    for (src, (meta, ctx)) in files.iter().zip(contexts.iter()) {
        if opts.dry_run {
            let planned = plan_multi_recipe_paths(
                &args.out,
                &recipes,
                ctx,
                Some(src.as_path()),
                args.overwrite,
            )?;
            for (r_idx, dest) in planned {
                outputs.push(json!({
                    "path": src,
                    "recipe_name": recipes[r_idx].name,
                    "dest": dest,
                    "status": "dry_run",
                }));
            }
            continue;
        }
        let img = load_file(src)?;
        let written = export_multi_recipe(
            &img,
            &meta.recipe,
            &args.out,
            &recipes,
            ctx,
            Some(src.as_path()),
            args.overwrite,
        )?;
        for item in written {
            outputs.push(json!({
                "path": src,
                "recipe_name": item.recipe_name,
                "dest": item.path,
                "width": item.width,
                "height": item.height,
                "status": "ok",
            }));
        }
    }

    let elapsed_ms = start.elapsed().as_millis();
    let human = format!(
        "{} {} multi-recipe output(s) across {} photo(s) in {elapsed_ms}ms",
        if opts.dry_run {
            "[dry-run] Planned"
        } else {
            "Exported"
        },
        outputs.len(),
        files.len(),
    );
    emit_ok(
        opts,
        "export",
        json!({
            "multi_recipe": true,
            "total_photos": files.len(),
            "total_outputs": outputs.len(),
            "elapsed_ms": elapsed_ms,
            "outputs": outputs,
        }),
        &human,
    );
    Ok(exit_code::OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQ: AtomicU64 = AtomicU64::new(1);

    fn temp_test_dir(tag: &str) -> PathBuf {
        let id = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("rune_cli_{tag}_{}_{id}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write_test_png(path: &Path, width: u32, height: u32) {
        let img = rune_editor_decode::generate_demo_hdr_scene(width, height);
        export_image_with_options(
            &img,
            &DevelopRecipe::default(),
            path,
            &ExportOptions {
                format: ExportFormat::Png8,
                max_edge: None,
                respect_crop: false,
                output_sharpen: 0.0,
                ..ExportOptions::default()
            },
        )
        .expect("write test png");
    }

    fn default_edit_args(path: PathBuf) -> EditArgs {
        EditArgs {
            paths: vec![path],
            recursive: false,
            picks: false,
            min_rating: None,
            sets: Vec::new(),
            preset: None,
            mix: 1.0,
            auto: Vec::new(),
            wb_pick: None,
            wb_preset: None,
            profile: None,
            profile_strength: None,
            film_curve: None,
            processing_mode: None,
            pick_film_base: None,
            pick_skin: None,
            pick_color_slices: Vec::new(),
            vignette_method: None,
            copy_from: None,
            only: Vec::new(),
            patch: None,
            reset: None,
            exposure: None,
            contrast: None,
            highlights: None,
            shadows: None,
            temp: None,
            tint: None,
            vibrance: None,
            saturation: None,
            clarity: None,
            structure: None,
            dehaze: None,
            sharpen: None,
            denoise: None,
            aspect: None,
            agent: "test".to_owned(),
            force: false,
        }
    }

    fn test_runtime_opts() -> RuntimeOpts {
        RuntimeOpts {
            format: ResolvedFormat::Json,
            dry_run: false,
            quiet: true,
            verbose: false,
            backend: crate::BackendMode::Cpu,
        }
    }

    #[test]
    fn duplicate_export_paths_rejected_with_validation_error() {
        let raw_plus_jpeg = vec![
            (
                PathBuf::from("/tmp/shoot/DSC0001.ARW"),
                PathBuf::from("/tmp/out/DSC0001.jpg"),
            ),
            (
                PathBuf::from("/tmp/shoot/DSC0001.JPG"),
                PathBuf::from("/tmp/out/DSC0001.jpg"),
            ),
        ];
        let err = check_duplicate_export_paths(&raw_plus_jpeg).unwrap_err();
        assert_eq!(err.code, "DUPLICATE_OUTPUT_PATH");
        assert_eq!(err.exit, exit_code::VALIDATION);

        let distinct = vec![
            (
                PathBuf::from("/tmp/shoot/DSC0001.ARW"),
                PathBuf::from("/tmp/out/DSC0001.jpg"),
            ),
            (
                PathBuf::from("/tmp/shoot/DSC0002.ARW"),
                PathBuf::from("/tmp/out/DSC0002.jpg"),
            ),
        ];
        assert!(check_duplicate_export_paths(&distinct).is_ok());
    }

    #[test]
    fn four_five_aspect_patch_updates_crop_via_dimension_probe() {
        let dir = temp_test_dir("aspect_patch");
        let photo = dir.join("sample_160x100.png");
        write_test_png(&photo, 160, 100);

        let patch: RecipePatch =
            serde_json::from_str(r#"{"geometry":{"aspect_preset":"FourFive"}}"#)
                .expect("valid FourFive patch");
        let args = default_edit_args(photo.clone());
        let opts = test_runtime_opts();

        edit_single_photo(
            &photo,
            &args,
            &[],
            None,
            None,
            Some(&patch),
            None,
            None,
            None,
            &[],
            &opts,
        )
        .expect("edit_single_photo with FourFive patch should succeed");

        let meta = PhotoMeta::load_sidecar(&photo)
            .expect("load sidecar")
            .expect("sidecar should exist");
        assert_eq!(
            meta.recipe.geometry.aspect_preset,
            rune_editor_core::CropAspect::FourFive
        );
        assert_ne!(
            meta.recipe.geometry.crop,
            [0.0, 0.0, 1.0, 1.0],
            "FourFive patch must update geometry.crop using probed dimensions"
        );
        let [out_w, out_h] = meta.recipe.geometry.cropped_output_size(160, 100);
        let ratio = out_w as f32 / out_h as f32;
        assert!(
            (ratio - 0.8).abs() < 0.03,
            "expected ~4:5 (0.8) aspect ratio, got {out_w}x{out_h} ({ratio:.3})"
        );

        // Also verify `--set geometry.aspect_preset=1:1` probes dimensions and updates crop.
        let sets = vec!["geometry.aspect_preset=1:1".to_owned()];
        edit_single_photo(
            &photo,
            &args,
            &sets,
            None,
            None,
            None,
            None,
            None,
            None,
            &[],
            &opts,
        )
        .expect("edit_single_photo with --set geometry.aspect_preset=1:1 should succeed");
        let meta2 = PhotoMeta::load_sidecar(&photo)
            .expect("load sidecar")
            .expect("sidecar should exist");
        let [sq_w, sq_h] = meta2.recipe.geometry.cropped_output_size(160, 100);
        assert!((sq_w as i32 - sq_h as i32).abs() <= 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn batch_set_meta_persists_without_recipe_diff() {
        let dir = temp_test_dir("batch_set_meta");
        let photo = dir.join("frame.png");
        write_test_png(&photo, 64, 64);

        let job: BatchJob = serde_json::from_value(json!({
            "schema_version": 1,
            "inputs": { "paths": [photo] },
            "pipeline": [{
                "name": "cull_pick",
                "ops": [{ "op": "set_meta", "rating": 5, "flag": "Pick" }]
            }]
        }))
        .expect("valid batch job");

        let opts = test_runtime_opts();
        run_batch_item(&photo, &job, "test-agent", &opts).expect("batch item should succeed");

        let meta = PhotoMeta::load_sidecar(&photo)
            .expect("load sidecar")
            .expect("SetMeta-only batch must write sidecar");
        assert_eq!(meta.rating, 5);
        assert_eq!(meta.flag, CullFlag::Pick);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn phase1_cli_edit_inspect_schema_search_and_batch_end_to_end() {
        use rune_editor_core::{ClarityMethod, FilmCurve, GrainType, WbPreset};

        let dir = temp_test_dir("phase1_e2e");
        let photo = dir.join("scene.png");
        write_test_png(&photo, 96, 64);
        let opts = test_runtime_opts();

        // 1. Default inspect reports 5500 K Kelvin WB readout and default film_curve/wb_preset.
        let (insp0, human0) = inspect_photo_payload(
            &InspectArgs {
                path: photo.clone(),
                include: Vec::new(),
            },
            &opts,
        )
        .expect("inspect default");
        assert_eq!(insp0["wb_kelvin"], 5500);
        assert_eq!(insp0["wb_preset"], "as_shot");
        assert_eq!(insp0["film_curve"], "film_standard");
        assert!(human0.contains("5500 K"));

        // 2. `rd edit` shorthand `--wb-preset`, `--film-curve`, and Phase 1 `--set` dot-paths.
        let mut edit_args = default_edit_args(photo.clone());
        edit_args.wb_preset = Some("tungsten".to_owned());
        edit_args.film_curve = Some("film_extra_shadow".to_owned());
        edit_args.sets = vec![
            "curves.rgb=0,0;0.25,0.22;0.75,0.79;1,1".to_owned(),
            "levels.red.in_black=0.04".to_owned(),
            "levels.green.in_mid=0.12".to_owned(),
            "levels.blue.out_white=0.94".to_owned(),
            "wheels.master_wheel.hue_x=0.15".to_owned(),
            "wheels.master_wheel.luma=-0.05".to_owned(),
            "bw.enabled=true".to_owned(),
            "bw.split_shadow_hue=220".to_owned(),
            "bw.split_shadow_sat=0.25".to_owned(),
            "bw.split_highlight_hue=42".to_owned(),
            "bw.split_highlight_sat=0.18".to_owned(),
            "clarity_method=punch".to_owned(),
            "sharpen_radius=1.8".to_owned(),
            "sharpen_threshold=0.15".to_owned(),
            "sharpen_halo=0.3".to_owned(),
            "nr_color=0.4".to_owned(),
            "grain_type=silver_rich".to_owned(),
            "grain_size=0.6".to_owned(),
        ];
        assert_eq!(cmd_edit(edit_args, &opts).expect("cmd_edit phase1"), 0);

        let meta = PhotoMeta::load_sidecar(&photo)
            .expect("load sidecar")
            .expect("sidecar written");
        assert_eq!(meta.recipe.wb_preset, WbPreset::Tungsten);
        assert_eq!(meta.recipe.film_curve, FilmCurve::FilmExtraShadow);
        assert_eq!(meta.recipe.curves.rgb.len(), 4);
        assert!((meta.recipe.levels.red.in_black - 0.04).abs() < 1e-4);
        assert!((meta.recipe.levels.green.in_mid - 0.12).abs() < 1e-4);
        assert!((meta.recipe.levels.blue.out_white - 0.94).abs() < 1e-4);
        assert!((meta.recipe.master_wheel.hue_x - 0.15).abs() < 1e-4);
        assert!((meta.recipe.master_wheel.luma - (-0.05)).abs() < 1e-4);
        assert!(meta.recipe.bw.enabled);
        assert!((meta.recipe.bw.split_shadow_hue - 220.0).abs() < 1e-3);
        assert!((meta.recipe.bw.split_shadow_sat - 0.25).abs() < 1e-4);
        assert!((meta.recipe.bw.split_highlight_hue - 42.0).abs() < 1e-3);
        assert!((meta.recipe.bw.split_highlight_sat - 0.18).abs() < 1e-4);
        assert_eq!(meta.recipe.clarity_method, ClarityMethod::Punch);
        assert!((meta.recipe.sharpen_radius - 1.8).abs() < 1e-4);
        assert!((meta.recipe.sharpen_threshold - 0.15).abs() < 1e-4);
        assert!((meta.recipe.sharpen_halo - 0.3).abs() < 1e-4);
        assert!((meta.recipe.nr_color - 0.4).abs() < 1e-4);
        assert_eq!(meta.recipe.grain_type, GrainType::SilverRich);
        assert!((meta.recipe.grain_size - 0.6).abs() < 1e-4);

        // 3. Inspect now reflects tungsten Kelvin (< 5500 K) and updated preset/curve.
        let (insp1, human1) = inspect_photo_payload(
            &InspectArgs {
                path: photo.clone(),
                include: Vec::new(),
            },
            &opts,
        )
        .expect("inspect after edit");
        let tungsten_k = insp1["wb_kelvin"].as_u64().expect("wb_kelvin integer");
        assert!(
            tungsten_k < 4500,
            "tungsten wb_preset should lower Kelvin readout, got {tungsten_k}"
        );
        assert_eq!(insp1["wb_preset"], "tungsten");
        assert_eq!(insp1["film_curve"], "film_extra_shadow");
        assert!(human1.contains(&format!("{tungsten_k} K")));

        // 4. `rd edit --patch` and `rd batch run` with Phase 1 RecipePatch fields.
        let job: BatchJob = serde_json::from_value(json!({
            "schema_version": 1,
            "inputs": { "paths": [photo] },
            "pipeline": [{
                "name": "phase1_grade",
                "ops": [{
                    "op": "patch",
                    "film_curve": "film_high_contrast",
                    "wb_preset": "shade",
                    "clarity_method": "classic",
                    "sharpen_radius": 2.2,
                    "sharpen_threshold": 0.2,
                    "sharpen_halo": 0.4,
                    "nr_color": 0.35,
                    "grain_type": "cubic",
                    "grain_size": 0.55,
                    "wheels": { "master": { "hue_x": -0.1, "hue_y": 0.08 } },
                    "curves": { "rgb": [[0.0, 0.0], [0.5, 0.54], [1.0, 1.0]] },
                    "levels": { "red": { "in_black": 0.02 }, "blue": { "in_white": 0.96 } },
                    "bw": { "split_shadow_hue": 200.0, "split_highlight_sat": 0.22 }
                }]
            }]
        }))
        .expect("valid Phase 1 batch job");
        run_batch_item(&photo, &job, "phase1-test", &opts).expect("batch run Phase 1");

        let meta2 = PhotoMeta::load_sidecar(&photo)
            .expect("reload sidecar")
            .expect("sidecar exists");
        assert_eq!(meta2.recipe.film_curve, FilmCurve::FilmHighContrast);
        assert_eq!(meta2.recipe.wb_preset, WbPreset::Shade);
        assert_eq!(meta2.recipe.clarity_method, ClarityMethod::Classic);
        assert_eq!(meta2.recipe.grain_type, GrainType::Cubic);
        assert!((meta2.recipe.sharpen_radius - 2.2).abs() < 1e-4);
        assert!((meta2.recipe.nr_color - 0.35).abs() < 1e-4);
        assert!((meta2.recipe.levels.red.in_black - 0.02).abs() < 1e-4);
        assert!((meta2.recipe.levels.blue.in_white - 0.96).abs() < 1e-4);
        assert!((meta2.recipe.master_wheel.hue_x - (-0.1)).abs() < 1e-4);
        assert!((meta2.recipe.bw.split_shadow_hue - 200.0).abs() < 1e-3);
        assert_eq!(meta2.recipe.curves.rgb.len(), 3);

        // 5. Verify `search_params` discovers all new Phase 1 parameter keys.
        for needle in [
            "film_curve",
            "wb_preset",
            "curves.rgb",
            "levels.red.in_black",
            "master_wheel.hue_x",
            "bw.split_shadow_hue",
            "clarity_method",
            "sharpen_radius",
            "sharpen_threshold",
            "sharpen_halo",
            "nr_color",
            "grain_type",
            "grain_size",
        ] {
            let hits = search_params(needle);
            assert!(
                hits.iter().any(|d| d.id == needle),
                "rd cli search missing Phase 1 parameter '{needle}'"
            );
        }

        // 6. Verify `--wb-preset` and `--film-curve` run BEFORE `--patch` (1:1 with MCP `rune_edit_photo`).
        let mut prec_args = default_edit_args(photo.clone());
        prec_args.wb_preset = Some("daylight".to_owned());
        prec_args.film_curve = Some("linear_response".to_owned());
        prec_args.patch = Some(r#"{"tint": 0.17, "film_curve": "film_standard"}"#.to_owned());
        assert_eq!(cmd_edit(prec_args, &opts).expect("cmd_edit precedence"), 0);
        let meta3 = PhotoMeta::load_sidecar(&photo)
            .expect("reload sidecar")
            .expect("sidecar exists");
        assert!(
            (meta3.recipe.tint - 0.17).abs() < 1e-4,
            "patch tint must override --wb-preset default tint (got {})",
            meta3.recipe.tint
        );
        assert_eq!(
            meta3.recipe.film_curve,
            FilmCurve::FilmStandard,
            "patch film_curve must override --film-curve flag to match MCP precedence"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn phase2_cli_color_editor_skin_lens_keystone_end_to_end() {
        use rune_editor_core::VignetteMethod;

        let dir = temp_test_dir("phase2_e2e");
        let photo = dir.join("architecture.png");
        write_test_png(&photo, 96, 64);
        let opts = test_runtime_opts();

        // 1. Reject invalid slice index / vignette method loudly.
        let mut bad_slice = default_edit_args(photo.clone());
        bad_slice.pick_color_slices = vec!["4:0.5,0.5".to_owned()];
        assert!(cmd_edit(bad_slice, &opts).is_err());

        let mut bad_vm = default_edit_args(photo.clone());
        bad_vm.vignette_method = Some("square_crop".to_owned());
        assert!(cmd_edit(bad_vm, &opts).is_err());

        // 2. `rd edit` with `--pick-skin`, `--pick-color-slice`, `--vignette-method`,
        //    plus `--patch` (running AFTER picks/presets, matching MCP 1:1) and `--set`.
        let mut edit_args = default_edit_args(photo.clone());
        edit_args.pick_skin = Some("0.55,0.45".to_owned());
        edit_args.pick_color_slices = vec!["0:0.25,0.20".to_owned(), "2:0.75,0.65".to_owned()];
        edit_args.vignette_method = Some("circular".to_owned());
        edit_args.patch = Some(
            json!({
                "vignette": -0.35,
                "vignette_method": "circular_on_crop",
                "skin_tone": {
                    "uniformity_hue": 0.55,
                    "uniformity_sat": 0.35,
                    "amount_lightness": 0.12
                },
                "advanced_color": {
                    "0": { "hue_range": 42.0, "uniformity": 0.5, "saturation": 0.25 }
                },
                "lens": {
                    "distortion": -0.22,
                    "chromatic_aberration": 0.30,
                    "purple_fringing": 0.45,
                    "light_falloff": 0.25
                },
                "geometry": {
                    "keystone_vertical": 0.28,
                    "keystone_horizontal": -0.14,
                    "keystone_aspect": 0.15,
                    "keystone_amount": 0.85
                }
            })
            .to_string(),
        );
        edit_args.sets = vec![
            "advanced_color.2.lightness=-0.18".to_owned(),
            "skin_tone.smoothness=0.6".to_owned(),
            "lens.light_falloff=0.32".to_owned(),
            "geometry.keystone_amount=0.9".to_owned(),
        ];
        assert_eq!(cmd_edit(edit_args, &opts).expect("cmd_edit phase2"), 0);

        let meta = PhotoMeta::load_sidecar(&photo)
            .expect("load sidecar")
            .expect("sidecar written");
        // Picked slices & skin tone were enabled before patch/set refined them:
        assert!(meta.recipe.skin_tone.enabled);
        assert!((meta.recipe.skin_tone.uniformity_hue - 0.55).abs() < 1e-4);
        assert!((meta.recipe.skin_tone.uniformity_sat - 0.35).abs() < 1e-4);
        assert!((meta.recipe.skin_tone.amount_lightness - 0.12).abs() < 1e-4);
        assert!((meta.recipe.skin_tone.smoothness - 0.6).abs() < 1e-4);
        assert!(meta.recipe.advanced_color[0].enabled);
        assert!((meta.recipe.advanced_color[0].hue_range - 42.0).abs() < 1e-3);
        assert!((meta.recipe.advanced_color[0].uniformity - 0.5).abs() < 1e-4);
        assert!((meta.recipe.advanced_color[0].saturation - 0.25).abs() < 1e-4);
        assert!(meta.recipe.advanced_color[2].enabled);
        assert!((meta.recipe.advanced_color[2].lightness - (-0.18)).abs() < 1e-4);
        // Precedence: patch vignette_method overrode --vignette-method flag:
        assert_eq!(meta.recipe.vignette_method, VignetteMethod::CircularOnCrop);
        assert!((meta.recipe.lens.distortion - (-0.22)).abs() < 1e-4);
        assert!((meta.recipe.lens.chromatic_aberration - 0.30).abs() < 1e-4);
        assert!((meta.recipe.lens.purple_fringing - 0.45).abs() < 1e-4);
        assert!((meta.recipe.lens.light_falloff - 0.32).abs() < 1e-4);
        assert!((meta.recipe.geometry.keystone_vertical - 0.28).abs() < 1e-4);
        assert!((meta.recipe.geometry.keystone_horizontal - (-0.14)).abs() < 1e-4);
        assert!((meta.recipe.geometry.keystone_aspect - 0.15).abs() < 1e-4);
        assert!((meta.recipe.geometry.keystone_amount - 0.9).abs() < 1e-4);

        // 3. `rd inspect` surfaces Phase 2 overrides cleanly.
        let (insp, _) = inspect_photo_payload(
            &InspectArgs {
                path: photo.clone(),
                include: Vec::new(),
            },
            &opts,
        )
        .expect("inspect phase2");
        let overrides = insp["recipe_overrides"]
            .as_object()
            .expect("overrides object");
        assert_eq!(overrides["vignette_method"], "circular_on_crop");
        assert!(overrides["lens"]["distortion"].as_f64().is_some());
        assert!(
            overrides["geometry"]["keystone_vertical"]
                .as_f64()
                .is_some()
        );
        assert_eq!(overrides["skin_tone"]["enabled"], true);
        assert_eq!(overrides["advanced_color"]["0"]["enabled"], true);

        // 4. `rd batch run` applies Phase 2 RecipePatch fields cleanly.
        let job: BatchJob = serde_json::from_value(json!({
            "schema_version": 1,
            "inputs": { "paths": [photo] },
            "pipeline": [{
                "name": "phase2_batch",
                "ops": [{
                    "op": "patch",
                    "vignette_method": "circular",
                    "advanced_color": { "1": { "enabled": true, "target_hue": 140.0, "uniformity": 0.4 } },
                    "skin_tone": { "uniformity_lightness": 0.3 },
                    "lens": { "purple_fringing": 0.6 },
                    "geometry": { "keystone_vertical": -0.2 }
                }]
            }]
        }))
        .expect("valid Phase 2 batch job");
        run_batch_item(&photo, &job, "phase2-test", &opts).expect("batch run Phase 2");
        let meta2 = PhotoMeta::load_sidecar(&photo)
            .expect("reload sidecar")
            .expect("sidecar exists");
        assert_eq!(meta2.recipe.vignette_method, VignetteMethod::Circular);
        assert!(meta2.recipe.advanced_color[1].enabled);
        assert!((meta2.recipe.advanced_color[1].target_hue - 140.0).abs() < 1e-3);
        assert!((meta2.recipe.skin_tone.uniformity_lightness - 0.3).abs() < 1e-4);
        assert!((meta2.recipe.lens.purple_fringing - 0.6).abs() < 1e-4);
        assert!((meta2.recipe.geometry.keystone_vertical - (-0.2)).abs() < 1e-4);

        // 5. `rd cli search` (`search_params`) discovers all Phase 2 parameter keys.
        for needle in [
            "advanced_color.0.enabled",
            "advanced_color.0.uniformity",
            "advanced_color.3.target_hue",
            "skin_tone.enabled",
            "skin_tone.uniformity_hue",
            "skin_tone.amount_sat",
            "vignette_method",
            "lens.distortion",
            "lens.chromatic_aberration",
            "lens.purple_fringing",
            "lens.light_falloff",
            "geometry.keystone_vertical",
            "geometry.keystone_horizontal",
            "geometry.keystone_aspect",
            "geometry.keystone_amount",
        ] {
            let hits = search_params(needle);
            assert!(
                hits.iter().any(|d| d.id == needle),
                "rd cli search missing Phase 2 parameter '{needle}'"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn phase3_cli_variants_styles_tags_clipboard_and_multi_export() {
        let dir = temp_test_dir("phase3_e2e");
        let styles_dir = dir.join("styles");
        let export_dir = dir.join("out");
        let photo_a = dir.join("studio_a.png");
        let photo_b = dir.join("studio_b.png");
        write_test_png(&photo_a, 96, 64);
        write_test_png(&photo_b, 96, 64);
        let opts = test_runtime_opts();

        // 1. `rd cull --color-tag` + `rd inspect` color_tag & variants summary.
        assert_eq!(
            cmd_cull(
                CullArgs {
                    paths: vec![photo_a.clone()],
                    from_stdin: false,
                    rate: Some(4),
                    pick: true,
                    reject: false,
                    unflag: false,
                    color_tag: Some("purple".to_owned()),
                },
                &opts,
            )
            .expect("cull with color_tag"),
            0
        );
        let (insp, human) = inspect_photo_payload(
            &InspectArgs {
                path: photo_a.clone(),
                include: Vec::new(),
            },
            &opts,
        )
        .expect("inspect after cull");
        assert_eq!(insp["color_tag"], "purple");
        assert_eq!(insp["active_variant_index"], 0);
        assert_eq!(insp["variant_count"], 1);
        assert!(human.contains("Tag: Purple"));
        assert!(human.contains("Variant: V1/1"));

        // 2. `rd session scan --color-tag purple --search studio_a` filters accurately.
        assert_eq!(
            cmd_session_scan(
                &dir,
                false,
                true,
                false,
                Some(4),
                Some("purple".to_owned()),
                Some("studio_a".to_owned()),
                Vec::new(),
                &opts,
            )
            .expect("session scan filtered"),
            0
        );

        // 3. `rd variant new / clone / select / delete` lifecycle.
        let mut edit_a = default_edit_args(photo_a.clone());
        edit_a.sets = vec!["exposure=0.5".to_owned(), "contrast=0.2".to_owned()];
        assert_eq!(cmd_edit(edit_a, &opts).expect("edit v1"), 0);

        assert_eq!(
            cmd_variant(
                VariantCmd::Clone {
                    photo: photo_a.clone(),
                    label: Some("Warm Alt".to_owned()),
                },
                &opts,
            )
            .expect("variant clone"),
            0
        );
        assert_eq!(
            cmd_variant(
                VariantCmd::New {
                    photo: photo_a.clone(),
                    label: Some("Clean Reset".to_owned()),
                },
                &opts,
            )
            .expect("variant new"),
            0
        );
        let meta_v = PhotoMeta::load_sidecar(&photo_a).unwrap().unwrap();
        assert_eq!(meta_v.variant_count(), 3);
        assert_eq!(meta_v.active_variant_index, 2);
        assert!(meta_v.recipe.exposure.abs() < 1e-4);

        assert_eq!(
            cmd_variant(
                VariantCmd::Select {
                    photo: photo_a.clone(),
                    index: 1,
                },
                &opts,
            )
            .expect("variant select"),
            0
        );
        let meta_sel = PhotoMeta::load_sidecar(&photo_a).unwrap().unwrap();
        assert_eq!(meta_sel.active_variant_index, 1);
        assert!((meta_sel.recipe.exposure - 0.5).abs() < 1e-4);

        assert_eq!(
            cmd_variant(
                VariantCmd::Delete {
                    photo: photo_a.clone(),
                    index: 2,
                },
                &opts,
            )
            .expect("variant delete"),
            0
        );
        assert_eq!(
            PhotoMeta::load_sidecar(&photo_a)
                .unwrap()
                .unwrap()
                .variant_count(),
            2
        );

        // 4. `rd style save / list / apply --stack / delete` + traversal rejection.
        assert!(
            cmd_style(
                StyleCmd::Save {
                    photo: photo_a.clone(),
                    name: "../escape".to_owned(),
                    description: String::new(),
                    groups: Vec::new(),
                    autoselect_adjusted: true,
                    styles_dir: Some(styles_dir.clone()),
                },
                &opts,
            )
            .is_err(),
            "traversal style name MUST be rejected"
        );
        assert_eq!(
            cmd_style(
                StyleCmd::Save {
                    photo: photo_a.clone(),
                    name: "Editorial Warm".to_owned(),
                    description: "Punchy studio look".to_owned(),
                    groups: Vec::new(),
                    autoselect_adjusted: true,
                    styles_dir: Some(styles_dir.clone()),
                },
                &opts,
            )
            .expect("style save"),
            0
        );
        assert_eq!(
            cmd_style(
                StyleCmd::List {
                    styles_dir: Some(styles_dir.clone()),
                },
                &opts,
            )
            .expect("style list"),
            0
        );
        // Apply stacked style onto photo_b (which has temperature=0.3 already).
        let mut edit_b = default_edit_args(photo_b.clone());
        edit_b.sets = vec!["temperature=0.3".to_owned()];
        assert_eq!(cmd_edit(edit_b, &opts).expect("seed photo_b"), 0);
        assert_eq!(
            cmd_style(
                StyleCmd::Apply {
                    name: "Editorial Warm".to_owned(),
                    paths: vec![photo_b.clone()],
                    stack: true,
                    strength: 0.5,
                    styles_dir: Some(styles_dir.clone()),
                },
                &opts,
            )
            .expect("style apply stack"),
            0
        );
        let meta_b = PhotoMeta::load_sidecar(&photo_b).unwrap().unwrap();
        assert!(
            (meta_b.recipe.temperature - 0.3).abs() < 1e-4,
            "stacked style must preserve unrelated adjustments"
        );
        assert!(
            (meta_b.recipe.exposure - 0.25).abs() < 1e-4,
            "50% stacked style should add +0.25 EV (got {})",
            meta_b.recipe.exposure
        );

        assert_eq!(
            cmd_style(
                StyleCmd::Delete {
                    name: "Editorial Warm".to_owned(),
                    styles_dir: Some(styles_dir.clone()),
                },
                &opts,
            )
            .expect("style delete"),
            0
        );

        // 5. `rd copy-look --autoselect-adjusted` selective clipboard copy.
        let mut edit_a_wb = default_edit_args(photo_a.clone());
        edit_a_wb.sets = vec!["tint=-0.12".to_owned()];
        assert_eq!(cmd_edit(edit_a_wb, &opts).expect("add tint on a"), 0);
        assert_eq!(
            cmd_copy_look(
                CopyLookArgs {
                    src: photo_a.clone(),
                    dst: vec![photo_b.clone()],
                    groups: vec!["wb".to_owned()],
                    autoselect_adjusted: false,
                    recursive: false,
                },
                &opts,
            )
            .expect("copy-look wb only"),
            0
        );
        let meta_b2 = PhotoMeta::load_sidecar(&photo_b).unwrap().unwrap();
        assert!((meta_b2.recipe.tint - (-0.12)).abs() < 1e-4);
        assert!(
            (meta_b2.recipe.exposure - 0.25).abs() < 1e-4,
            "wb-only copy-look must not overwrite tone exposure"
        );

        // 6. `rd export` with `--recipes`, `--naming-template`, `--watermark`, `--strip-metadata`, `--color-space display-p3`.
        assert_eq!(
            cmd_export(
                ExportArgs {
                    paths: vec![photo_a.clone()],
                    out: export_dir.clone(),
                    to: "jpeg".to_owned(),
                    quality: 88,
                    max_edge: None,
                    output_sharpen: 0.1,
                    no_crop: false,
                    naming: "{stem}.{ext}".to_owned(),
                    naming_template: Some("{ImageName}_{Recipe}_{Variant}".to_owned()),
                    watermark: Some("© Rune Studio".to_owned()),
                    watermark_opacity: 0.5,
                    watermark_position: "bottom_right".to_owned(),
                    watermark_scale: 1.0,
                    strip_metadata: true,
                    color_space: "display-p3".to_owned(),
                    recipes: Some("full_res_jpeg,web_2048px".to_owned()),
                    recursive: false,
                    picks: false,
                    min_rating: None,
                    skip_existing: false,
                    overwrite: false,
                    jobs: 2,
                },
                &opts,
            )
            .expect("multi-recipe export"),
            0
        );
        let full_out = export_dir.join("studio_a_Full-Res_JPEG_V2.jpg");
        let web_out = export_dir.join("web").join("studio_a_Web_2048px_V2.jpg");
        assert!(full_out.exists(), "expected {}", full_out.display());
        assert!(web_out.exists(), "expected {}", web_out.display());

        // Re-running without --overwrite must fail cleanly!
        assert!(
            cmd_export(
                ExportArgs {
                    paths: vec![photo_a.clone()],
                    out: export_dir.clone(),
                    to: "jpeg".to_owned(),
                    quality: 88,
                    max_edge: None,
                    output_sharpen: 0.0,
                    no_crop: false,
                    naming: "{stem}.{ext}".to_owned(),
                    naming_template: Some("{ImageName}_{Recipe}_{Variant}".to_owned()),
                    watermark: None,
                    watermark_opacity: 0.45,
                    watermark_position: "bottom_right".to_owned(),
                    watermark_scale: 1.0,
                    strip_metadata: false,
                    color_space: "srgb".to_owned(),
                    recipes: Some("full_res_jpeg,web_2048px".to_owned()),
                    recursive: false,
                    picks: false,
                    min_rating: None,
                    skip_existing: false,
                    overwrite: false,
                    jobs: 2,
                },
                &opts,
            )
            .is_err(),
            "multi-recipe export without --overwrite must reject existing destinations"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn phase4_cli_layers_masks_ai_subject_and_heal_end_to_end() {
        use rune_editor_core::{HealMode, MaskCombineOp};

        let dir = temp_test_dir("phase4_e2e");
        let photo = dir.join("portrait.png");
        write_test_png(&photo, 96, 64);
        let opts = test_runtime_opts();

        // 1. Add linear gradient layer with local adjustments.
        assert_eq!(
            cmd_layer(
                LayerCmd::Add {
                    photo: photo.clone(),
                    name: "Sky Burn".to_owned(),
                    kind: "linear".to_owned(),
                    opacity: 0.85,
                    sets: vec!["exposure=-0.6".to_owned(), "dehaze=0.25".to_owned()],
                    engine: None,
                    points: Vec::new(),
                    bbox: None,
                    invert: false,
                },
                &opts,
            )
            .expect("layer add linear"),
            0
        );

        // 2. Add Combine Subtract radial mask + Luma Intersect mask onto layer #0.
        assert_eq!(
            cmd_layer(
                LayerCmd::MaskAdd {
                    photo: photo.clone(),
                    index: "0".to_owned(),
                    kind: "radial".to_owned(),
                    op: "subtract".to_owned(),
                    start: None,
                    end: None,
                    center: Some("0.5,0.5".to_owned()),
                    radius: Some("0.25,0.20".to_owned()),
                    angle: 15.0,
                    feather: Some(0.45),
                    low: 0.0,
                    high: 1.0,
                    low_falloff: 0.1,
                    high_falloff: 0.1,
                    inverted: false,
                    engine: None,
                    points: Vec::new(),
                    bbox: None,
                    invert: false,
                },
                &opts,
            )
            .expect("layer mask-add radial subtract"),
            0
        );
        assert_eq!(
            cmd_layer(
                LayerCmd::MaskAdd {
                    photo: photo.clone(),
                    index: "layer-1".to_owned(),
                    kind: "luma".to_owned(),
                    op: "intersect".to_owned(),
                    start: None,
                    end: None,
                    center: None,
                    radius: None,
                    angle: 0.0,
                    feather: None,
                    low: 0.45,
                    high: 1.0,
                    low_falloff: 0.12,
                    high_falloff: 0.08,
                    inverted: false,
                    engine: None,
                    points: Vec::new(),
                    bbox: None,
                    invert: false,
                },
                &opts,
            )
            .expect("layer mask-add luma intersect"),
            0
        );

        // 3. Add AI Subject & AI Background layers.
        assert_eq!(
            cmd_layer(
                LayerCmd::Add {
                    photo: photo.clone(),
                    name: "Subject Pop".to_owned(),
                    kind: "ai-subject".to_owned(),
                    opacity: 1.0,
                    sets: vec!["exposure=0.35".to_owned(), "clarity=0.2".to_owned()],
                    engine: None,
                    points: Vec::new(),
                    bbox: None,
                    invert: false,
                },
                &opts,
            )
            .expect("layer add ai-subject"),
            0
        );
        assert_eq!(
            cmd_layer(
                LayerCmd::Add {
                    photo: photo.clone(),
                    name: "Bg Dim".to_owned(),
                    kind: "ai-background".to_owned(),
                    opacity: 0.75,
                    sets: vec!["exposure=-0.4".to_owned()],
                    engine: None,
                    points: Vec::new(),
                    bbox: None,
                    invert: false,
                },
                &opts,
            )
            .expect("layer add ai-background"),
            0
        );

        // 4. Update layer #0 opacity/inverted/local adjustment.
        assert_eq!(
            cmd_layer(
                LayerCmd::Update {
                    photo: photo.clone(),
                    index: "0".to_owned(),
                    name: Some("Sky Burn v2".to_owned()),
                    opacity: Some(0.9),
                    enabled: Some(true),
                    inverted: Some(false),
                    sets: vec!["exposure+=0.1".to_owned()],
                },
                &opts,
            )
            .expect("layer update"),
            0
        );

        // 5. Add Spot Heal (auto-source) & Clone stamps.
        assert_eq!(
            cmd_heal(
                HealCmd::Add {
                    photo: photo.clone(),
                    target: "0.42,0.38".to_owned(),
                    source: None,
                    radius: 0.03,
                    feather: 0.4,
                    opacity: 1.0,
                    mode: "heal".to_owned(),
                },
                &opts,
            )
            .expect("heal add"),
            0
        );
        assert_eq!(
            cmd_heal(
                HealCmd::Add {
                    photo: photo.clone(),
                    target: "0.70,0.60".to_owned(),
                    source: Some("0.60,0.60".to_owned()),
                    radius: 0.04,
                    feather: 0.35,
                    opacity: 0.9,
                    mode: "clone".to_owned(),
                },
                &opts,
            )
            .expect("clone add"),
            0
        );

        // 6. Verify persisted sidecar state & `rd inspect` summary.
        let meta = PhotoMeta::load_sidecar(&photo).unwrap().unwrap();
        assert_eq!(meta.recipe.layers.len(), 3);
        assert_eq!(meta.recipe.layers[0].name, "Sky Burn v2");
        assert!((meta.recipe.layers[0].opacity - 0.9).abs() < 1e-4);
        assert!((meta.recipe.layers[0].adjustments.exposure - (-0.5)).abs() < 1e-4);
        assert_eq!(meta.recipe.layers[0].masks.len(), 3);
        assert_eq!(meta.recipe.layers[0].masks[1].op, MaskCombineOp::Subtract);
        assert_eq!(meta.recipe.layers[0].masks[2].op, MaskCombineOp::Intersect);
        assert_eq!(meta.recipe.layers[1].primary_mask_badge(), "AI Mask");
        assert_eq!(meta.recipe.heal_spots.len(), 2);
        assert_eq!(meta.recipe.heal_spots[0].mode, HealMode::Heal);
        assert_eq!(meta.recipe.heal_spots[1].mode, HealMode::Clone);

        let (insp, human) = inspect_photo_payload(
            &InspectArgs {
                path: photo.clone(),
                include: Vec::new(),
            },
            &opts,
        )
        .expect("inspect phase4");
        assert_eq!(insp["layer_count"], 3);
        assert_eq!(insp["layers"].as_array().unwrap().len(), 3);
        assert_eq!(insp["heal_spot_count"], 2);
        assert!(human.contains("Layers: 3 | Heal Spots: 2"));

        // 7. MaskClear, Layer Remove, Heal Remove & Clear.
        assert_eq!(
            cmd_layer(
                LayerCmd::MaskClear {
                    photo: photo.clone(),
                    index: "0".to_owned(),
                },
                &opts,
            )
            .expect("mask clear"),
            0
        );
        assert_eq!(
            cmd_layer(
                LayerCmd::Remove {
                    photo: photo.clone(),
                    index: "2".to_owned(),
                },
                &opts,
            )
            .expect("layer remove"),
            0
        );
        assert_eq!(
            cmd_heal(
                HealCmd::Remove {
                    photo: photo.clone(),
                    index: "0".to_owned(),
                },
                &opts,
            )
            .expect("heal remove"),
            0
        );
        assert_eq!(
            cmd_heal(
                HealCmd::Clear {
                    photo: photo.clone(),
                },
                &opts,
            )
            .expect("heal clear"),
            0
        );
        let meta_final = PhotoMeta::load_sidecar(&photo).unwrap().unwrap();
        assert_eq!(meta_final.recipe.layers.len(), 2);
        assert_eq!(meta_final.recipe.layers[0].masks.len(), 0);
        assert_eq!(meta_final.recipe.heal_spots.len(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn phase5_cli_film_negative_session_init_and_hot_folder_end_to_end() {
        let dir = std::env::temp_dir().join(format!(
            "rune_cli_phase5_film_neg_session_hot_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");
        let photo = dir.join("neg_scan.png");
        write_test_png(&photo, 64, 48);
        let opts = test_runtime_opts();

        // 1. `rd edit` with `--processing-mode`, `--pick-film-base` BEFORE `--patch` & `--set`!
        let mut args = default_edit_args(photo.clone());
        args.processing_mode = Some("film_negative".to_owned());
        args.pick_film_base = Some("0.5,0.5".to_owned());
        args.patch = Some(r#"{"film_negative":{"density":0.25,"black_clip":0.04}}"#.to_owned());
        args.sets = vec!["film_negative.contrast=0.18".to_owned()];
        assert_eq!(cmd_edit(args, &opts).expect("phase5 edit"), 0);

        let meta = PhotoMeta::load_sidecar(&photo).unwrap().unwrap();
        assert_eq!(meta.recipe.processing_mode, ProcessingMode::FilmNegative);
        assert!((meta.recipe.film_negative.density - 0.25).abs() < 1e-4);
        assert!((meta.recipe.film_negative.contrast - 0.18).abs() < 1e-4);
        assert!((meta.recipe.film_negative.black_clip - 0.04).abs() < 1e-4);
        // Picked base RGB should have been updated away from default C-41 mask values.
        assert!(
            (meta.recipe.film_negative.base_r - 0.95).abs() > 1e-3
                || (meta.recipe.film_negative.base_b - 0.34).abs() > 1e-3
        );

        // 2. `rd inspect` exposes `processing_mode` and `film_negative` in JSON & TTY text.
        let (insp, human) = inspect_photo_payload(
            &InspectArgs {
                path: photo.clone(),
                include: Vec::new(),
            },
            &opts,
        )
        .expect("inspect phase5");
        assert_eq!(insp["processing_mode"], "film_negative");
        assert!((insp["film_negative"]["density"].as_f64().unwrap() - 0.25).abs() < 1e-4);
        assert!(human.contains("Mode: Film Negative"));
        assert!(human.contains("Film Negative: base="));

        // 3. `rd session init <dir>` creates Capture One 4-folder session layout.
        let session_dir = dir.join("studio_session");
        assert_eq!(
            cmd_session_init(&session_dir, &opts).expect("session init"),
            0
        );
        assert!(session_dir.join("Capture").is_dir());
        assert!(session_dir.join("Selects").is_dir());
        assert!(session_dir.join("Output").is_dir());
        assert!(session_dir.join("Trash").is_dir());

        // 4. `rd session watch <dir> --once --style warm_film` ingests photos & persists sidecars.
        let hot_photo = session_dir.join("Capture").join("incoming_01.png");
        write_test_png(&hot_photo, 64, 48);
        assert!(!DevelopRecipe::sidecar_path_for(&hot_photo).exists());
        assert_eq!(
            cmd_session_watch(
                &session_dir.join("Capture"),
                Some("warm_film"),
                true,
                15,
                &opts,
            )
            .expect("session watch once"),
            0
        );
        let ingested_meta = PhotoMeta::load_sidecar(&hot_photo)
            .expect("load hot sidecar")
            .expect("hot sidecar created");
        assert_eq!(
            ingested_meta.recipe.preset_name.as_deref(),
            Some("Warm Film")
        );
        assert!(!ingested_meta.history.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn phase3_cli_camera_profiles_import_list_edit_and_export_end_to_end() {
        let dir = temp_test_dir("phase3_camera_profiles");
        let profiles_dir = dir.join("profiles");
        let photo = dir.join("portrait_raw.png");
        write_test_png(&photo, 96, 64);
        let opts = test_runtime_opts();

        // Build a valid synthetic 2x2x2 warm-tinted `.cube` 3D LUT on disk.
        let cube_path = dir.join("WarmStudio.cube");
        let cube_text = "\
TITLE \"Warm Studio LUT\"
LUT_3D_SIZE 2
DOMAIN_MIN 0.0 0.0 0.0
DOMAIN_MAX 1.0 1.0 1.0
0.02 0.00 0.00
1.00 0.04 0.00
0.02 0.96 0.00
1.00 0.98 0.02
0.02 0.00 0.90
1.00 0.05 0.90
0.02 0.96 0.90
1.00 0.98 0.92
";
        std::fs::write(&cube_path, cube_text).expect("write synthetic .cube");

        // 1. `rd profile import` rejects unsupported extensions & traversal names cleanly.
        let bad_ext = dir.join("evil.txt");
        std::fs::write(&bad_ext, b"not a profile").expect("write bad ext");
        assert!(
            cmd_profile(
                ProfileCmd::Import {
                    file: bad_ext,
                    name: None,
                    overwrite: false,
                    profiles_dir: Some(profiles_dir.clone()),
                },
                &opts,
            )
            .is_err(),
            "non-profile extension MUST be rejected"
        );
        assert!(
            cmd_profile(
                ProfileCmd::Import {
                    file: cube_path.clone(),
                    name: Some("../escape".to_owned()),
                    overwrite: false,
                    profiles_dir: Some(profiles_dir.clone()),
                },
                &opts,
            )
            .is_err(),
            "traversal --name MUST be rejected"
        );

        // 2. Import synthetic `.cube` profile; duplicate without `--overwrite` fails; `--overwrite` succeeds.
        assert_eq!(
            cmd_profile(
                ProfileCmd::Import {
                    file: cube_path.clone(),
                    name: Some("warm_studio".to_owned()),
                    overwrite: false,
                    profiles_dir: Some(profiles_dir.clone()),
                },
                &opts,
            )
            .expect("import .cube profile"),
            0
        );
        assert!(
            cmd_profile(
                ProfileCmd::Import {
                    file: cube_path.clone(),
                    name: Some("warm_studio".to_owned()),
                    overwrite: false,
                    profiles_dir: Some(profiles_dir.clone()),
                },
                &opts,
            )
            .is_err(),
            "duplicate import without --overwrite must fail"
        );
        assert_eq!(
            cmd_profile(
                ProfileCmd::Import {
                    file: cube_path,
                    name: Some("warm_studio".to_owned()),
                    overwrite: true,
                    profiles_dir: Some(profiles_dir.clone()),
                },
                &opts,
            )
            .expect("import .cube with --overwrite"),
            0
        );

        // 3. `rd profile list` returns all 14 bundled MIT profiles + imported user profile.
        let listed = list_all_camera_profiles(
            Some("Apple"),
            Some("iPhone 16 Pro"),
            Some(profiles_dir.as_path()),
        );
        assert_eq!(listed.len(), 15, "14 bundled + 1 imported user profile");
        assert!(listed.iter().any(|p| p.id == "pro_standard"));
        assert!(listed.iter().any(|p| p.id == "classic_chrome"));
        assert!(listed.iter().any(|p| p.id == "user:warm_studio"));
        // Verify unmatched user profile with overlapping substring camera_match never falsely highlights.
        let synthetic_user = CameraProfileDescriptor {
            id: "user:canon_r5".to_owned(),
            name: "Canon R5".to_owned(),
            category: ProfileCategory::User,
            description: "Test".to_owned(),
            camera_match: Some("Canon EOS R5".to_owned()),
        };
        assert!(!is_profile_exif_highlighted(
            &synthetic_user,
            14,
            0,
            Some("Canon"),
            Some("Canon EOS R50"),
        ));
        assert!(is_profile_exif_highlighted(
            &synthetic_user,
            0,
            1,
            Some("Canon"),
            Some("Canon EOS R5"),
        ));
        assert_eq!(
            cmd_profile(
                ProfileCmd::List {
                    camera: Some("iPhone 16 Pro".to_owned()),
                    make: Some("Apple".to_owned()),
                    profiles_dir: Some(profiles_dir.clone()),
                },
                &opts,
            )
            .expect("profile list"),
            0
        );

        // 4. `rd edit --profile` and `--profile-strength` validation & precedence (applied BEFORE `--patch`/`--set`).
        let mut bad_prof = default_edit_args(photo.clone());
        bad_prof.profile = Some("nonexistent_profile".to_owned());
        assert!(
            cmd_edit(bad_prof, &opts).is_err(),
            "unknown --profile must fail"
        );
        let mut bad_str = default_edit_args(photo.clone());
        bad_str.profile_strength = Some(1.5);
        assert!(
            cmd_edit(bad_str, &opts).is_err(),
            "out-of-range --profile-strength must fail"
        );

        // Top-level `--profile pro_standard --profile-strength 0.4` overridden by `--patch` + `--set`!
        let mut edit_prec = default_edit_args(photo.clone());
        edit_prec.profile = Some("pro_standard".to_owned());
        edit_prec.profile_strength = Some(0.4);
        edit_prec.patch = Some(r#"{"camera_profile":"classic_chrome"}"#.to_owned());
        edit_prec.sets = vec!["profile_strength=0.75".to_owned()];
        assert_eq!(cmd_edit(edit_prec, &opts).expect("edit precedence"), 0);

        let meta = PhotoMeta::load_sidecar(&photo).unwrap().unwrap();
        assert_eq!(meta.recipe.camera_profile, "classic_chrome");
        assert!((meta.recipe.profile_strength - 0.75).abs() < 1e-4);

        // 5. `rd inspect` exposes `camera_profile` and `profile_strength` in JSON & TTY text.
        let (insp, human) = inspect_photo_payload(
            &InspectArgs {
                path: photo.clone(),
                include: Vec::new(),
            },
            &opts,
        )
        .expect("inspect profile");
        assert_eq!(insp["camera_profile"], "classic_chrome");
        assert!((insp["profile_strength"].as_f64().unwrap() - 0.75).abs() < 1e-4);
        assert!(
            human.contains("Profile: classic_chrome (75%)"),
            "TTY inspect must expose camera_profile and strength, got: {human}"
        );

        // 6. `rd schema`, `rd cli search`, and `rd batch run` all support camera profiles.
        assert_eq!(cmd_schema("all", &opts).expect("schema all"), 0);
        assert_eq!(
            cmd_cli_search("pro_standard", &opts).expect("cli search"),
            0
        );
        let batch_job: BatchJob = serde_json::from_value(json!({
            "schema_version": 1,
            "inputs": { "paths": [photo] },
            "pipeline": [{
                "name": "apply_pro_standard",
                "ops": [{ "op": "patch", "camera_profile": "pro_standard", "profile_strength": 0.9 }]
            }]
        }))
        .expect("valid batch job with camera_profile");
        run_batch_item(&photo, &batch_job, "batch-test", &opts).expect("batch run profile");
        let meta_batch = PhotoMeta::load_sidecar(&photo).unwrap().unwrap();
        assert_eq!(meta_batch.recipe.camera_profile, "pro_standard");
        assert!((meta_batch.recipe.profile_strength - 0.9).abs() < 1e-4);

        // 7. `rd export` with `classic_chrome` vs `pro_standard` produces valid distinct developed outputs!
        let out_pro = dir.join("out_pro_standard.png");
        let out_chrome = dir.join("out_classic_chrome.png");
        assert_eq!(
            cmd_export(
                ExportArgs {
                    paths: vec![photo.clone()],
                    out: out_pro.clone(),
                    to: "png8".to_owned(),
                    quality: 92,
                    max_edge: None,
                    output_sharpen: 0.0,
                    no_crop: false,
                    naming: "{stem}.{ext}".to_owned(),
                    naming_template: None,
                    watermark: None,
                    watermark_opacity: 0.45,
                    watermark_position: "bottom_right".to_owned(),
                    watermark_scale: 1.0,
                    strip_metadata: false,
                    color_space: "srgb".to_owned(),
                    recipes: None,
                    recursive: false,
                    picks: false,
                    min_rating: None,
                    skip_existing: false,
                    overwrite: false,
                    jobs: 1,
                },
                &opts,
            )
            .expect("export pro_standard"),
            0
        );
        let mut edit_chrome = default_edit_args(photo.clone());
        edit_chrome.profile = Some("classic_chrome".to_owned());
        edit_chrome.profile_strength = Some(1.0);
        assert_eq!(
            cmd_edit(edit_chrome, &opts).expect("edit classic_chrome"),
            0
        );
        assert_eq!(
            cmd_export(
                ExportArgs {
                    paths: vec![photo.clone()],
                    out: out_chrome.clone(),
                    to: "png8".to_owned(),
                    quality: 92,
                    max_edge: None,
                    output_sharpen: 0.0,
                    no_crop: false,
                    naming: "{stem}.{ext}".to_owned(),
                    naming_template: None,
                    watermark: None,
                    watermark_opacity: 0.45,
                    watermark_position: "bottom_right".to_owned(),
                    watermark_scale: 1.0,
                    strip_metadata: false,
                    color_space: "srgb".to_owned(),
                    recipes: None,
                    recursive: false,
                    picks: false,
                    min_rating: None,
                    skip_existing: false,
                    overwrite: false,
                    jobs: 1,
                },
                &opts,
            )
            .expect("export classic_chrome"),
            0
        );
        let bytes_pro = std::fs::read(&out_pro).expect("read pro export");
        let bytes_chrome = std::fs::read(&out_chrome).expect("read chrome export");
        assert!(bytes_pro.len() > 100 && bytes_chrome.len() > 100);
        assert_ne!(
            bytes_pro, bytes_chrome,
            "pro_standard and classic_chrome exports must render distinct pixels"
        );

        // 8. `rd profile delete` removes user profile cleanly and returns NOT_FOUND on second call.
        assert_eq!(
            cmd_profile(
                ProfileCmd::Delete {
                    slug: "user:warm_studio".to_owned(),
                    profiles_dir: Some(profiles_dir.clone()),
                },
                &opts,
            )
            .expect("delete user profile"),
            0
        );
        assert!(
            cmd_profile(
                ProfileCmd::Delete {
                    slug: "warm_studio".to_owned(),
                    profiles_dir: Some(profiles_dir.clone()),
                },
                &opts,
            )
            .is_err(),
            "deleting already-deleted profile must return error"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn phase2_cli_models_and_ai_select_hermetic_end_to_end() {
        use std::sync::Mutex;

        use clap::Parser;
        use rune_editor_core::{
            LAYER_MASK_TILE_SIZE, MaskCombineOp, MaskShape, RUNE_MODELS_DIR_ENV,
            inspect_model_install,
        };

        static ENV_LOCK: Mutex<()> = Mutex::new(());
        struct ModelsDirGuard {
            prev: Option<std::ffi::OsString>,
        }
        impl Drop for ModelsDirGuard {
            fn drop(&mut self) {
                match &self.prev {
                    Some(v) => unsafe { std::env::set_var(RUNE_MODELS_DIR_ENV, v) },
                    None => unsafe { std::env::remove_var(RUNE_MODELS_DIR_ENV) },
                }
            }
        }

        let _guard_lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp_test_dir("phase2_models_ai_select");
        let models_dir = dir.join("models");
        std::fs::create_dir_all(&models_dir).expect("create models dir");
        let _env_guard = ModelsDirGuard {
            prev: std::env::var_os(RUNE_MODELS_DIR_ENV),
        };
        unsafe { std::env::set_var(RUNE_MODELS_DIR_ENV, &models_dir) };

        let opts = test_runtime_opts();

        // 1. Verify Clap parsing for `rd model` (incl. `download` alias) & `rd layer --kind ai-select`.
        let parsed_dl = Cli::try_parse_from([
            "rd",
            "model",
            "download",
            "sam3",
            "--accept-license",
            "--from-dir",
            "/tmp/fake-sam3",
        ])
        .expect("clap parse rd model download alias");
        assert!(matches!(
            parsed_dl.command,
            Command::Model {
                sub: ModelCmd::Setup {
                    accept_license: true,
                    from_dir: Some(_),
                    ..
                }
            }
        ));
        let parsed_layer = Cli::try_parse_from([
            "rd",
            "layer",
            "add",
            "photo.jpg",
            "--name",
            "Smart",
            "--kind",
            "ai-select",
            "--engine",
            "heuristic",
            "--point",
            "0.5,0.4,+",
            "--point",
            "0.1,0.1,-",
            "--box",
            "0.2,0.2,0.8,0.8",
            "--invert",
        ])
        .expect("clap parse rd layer add ai-select");
        assert!(matches!(
            parsed_layer.command,
            Command::Layer {
                sub: LayerCmd::Add { invert: true, .. }
            }
        ));

        // 2. `rd model list`, `rd model status`, `rd model use`.
        assert_eq!(cmd_model(ModelCmd::List, &opts).expect("model list"), 0);
        assert_eq!(cmd_model(ModelCmd::Status, &opts).expect("model status"), 0);
        assert_eq!(
            cmd_model(
                ModelCmd::Use {
                    engine: "heuristic".to_owned(),
                },
                &opts,
            )
            .expect("model use heuristic"),
            0
        );
        assert_eq!(
            load_ai_model_config().expect("load config").active_engine,
            SegmentationEngine::Heuristic
        );
        assert_eq!(
            cmd_model(
                ModelCmd::Use {
                    engine: "sam3".to_owned(),
                },
                &opts,
            )
            .expect("model use sam3"),
            0
        );
        assert_eq!(
            load_ai_model_config().expect("load config").active_engine,
            SegmentationEngine::Sam3Tracker
        );
        assert!(
            cmd_model(
                ModelCmd::Use {
                    engine: "nonexistent_engine".to_owned(),
                },
                &opts,
            )
            .is_err(),
            "unknown engine must fail"
        );

        // 3. Offline local directory setup for SAM 2.1 (Apache-2.0, no license flag required).
        let sam21_src = dir.join("sam21_local");
        std::fs::create_dir_all(&sam21_src).expect("create sam21 src");
        for f in ModelId::Sam21HieraTinyFp16.spec().files {
            std::fs::write(sam21_src.join(f.filename), b"\x08\x01ONNX_DUMMY_WEIGHTS")
                .expect("write dummy onnx");
        }
        assert_eq!(
            cmd_model(
                ModelCmd::Setup {
                    model: "sam2.1".to_owned(),
                    accept_license: false,
                    from_dir: Some(sam21_src.clone()),
                },
                &opts,
            )
            .expect("sam2.1 local setup"),
            0
        );
        assert!(inspect_model_install(ModelId::Sam21HieraTinyFp16).is_installed());

        // 4. SAM 3 license gate rejects without `--accept-license` with exact license name, URL, and hint!
        let sam3_src = dir.join("sam3_local");
        std::fs::create_dir_all(sam3_src.join("onnx")).expect("create sam3 onnx src");
        for f in ModelId::Sam3TrackerQ4f16.spec().files {
            std::fs::write(
                sam3_src.join("onnx").join(f.filename),
                b"\x08\x01SAM3_DUMMY_WEIGHTS",
            )
            .expect("write dummy sam3 onnx");
        }
        let lic_err = cmd_model(
            ModelCmd::Setup {
                model: "sam3-tracker-q4f16".to_owned(),
                accept_license: false,
                from_dir: Some(sam3_src.clone()),
            },
            &opts,
        )
        .expect_err("SAM 3 without --accept-license must fail");
        assert_eq!(lic_err.code, "LICENSE_REQUIRED");
        assert!(lic_err.message.contains("Meta SAM License"));
        assert!(
            lic_err
                .message
                .contains("https://github.com/facebookresearch/sam3/blob/main/LICENSE")
        );
        assert!(
            lic_err
                .hint
                .as_deref()
                .unwrap_or("")
                .contains("--accept-license")
        );
        assert!(!inspect_model_install(ModelId::Sam3TrackerQ4f16).is_installed());

        // With `--accept-license`, SAM 3 installs cleanly and persists license acceptance!
        assert_eq!(
            cmd_model(
                ModelCmd::Setup {
                    model: "sam3".to_owned(),
                    accept_license: true,
                    from_dir: Some(sam3_src),
                },
                &opts,
            )
            .expect("sam3 local setup with --accept-license"),
            0
        );
        assert!(inspect_model_install(ModelId::Sam3TrackerQ4f16).is_installed());
        assert!(
            load_ai_model_config()
                .expect("load config")
                .is_license_accepted(ModelId::Sam3TrackerQ4f16)
        );

        // 5. `rd model remove` removes installed weights cleanly.
        assert_eq!(
            cmd_model(
                ModelCmd::Remove {
                    model: "sam2.1".to_owned(),
                },
                &opts,
            )
            .expect("remove sam2.1"),
            0
        );
        assert!(!inspect_model_install(ModelId::Sam21HieraTinyFp16).is_installed());
        assert_eq!(
            cmd_model(
                ModelCmd::Remove {
                    model: "sam3".to_owned(),
                },
                &opts,
            )
            .expect("remove sam3"),
            0
        );
        assert!(!inspect_model_install(ModelId::Sam3TrackerQ4f16).is_installed());

        // Reset engine back to auto.
        assert_eq!(
            cmd_model(
                ModelCmd::Use {
                    engine: "auto".to_owned(),
                },
                &opts,
            )
            .expect("model use auto"),
            0
        );

        // 6. `rd schema` & `rd cli search` include `rd model` and `ai-select`.
        assert_eq!(cmd_schema("commands", &opts).expect("schema commands"), 0);
        assert_eq!(cmd_cli_search("model", &opts).expect("cli search model"), 0);
        assert_eq!(
            cmd_cli_search("ai-select", &opts).expect("cli search ai-select"),
            0
        );

        // 7. `rd layer add --kind ai-select` & `rd layer mask-add --kind ai-select`.
        let photo = dir.join("smart_select.png");
        write_test_png(&photo, 96, 64);

        // Missing points and box must fail cleanly.
        assert!(
            cmd_layer(
                LayerCmd::Add {
                    photo: photo.clone(),
                    name: "MissingPrompt".to_owned(),
                    kind: "ai-select".to_owned(),
                    opacity: 1.0,
                    sets: Vec::new(),
                    engine: Some("heuristic".to_owned()),
                    points: Vec::new(),
                    bbox: None,
                    invert: false,
                },
                &opts,
            )
            .is_err(),
            "ai-select without --point or --box must fail"
        );

        // Explicitly requesting an uninstalled SAM 2.1 model fails with MODEL_NOT_INSTALLED.
        let uninst_err = cmd_layer(
            LayerCmd::Add {
                photo: photo.clone(),
                name: "NeedsSam".to_owned(),
                kind: "ai-select".to_owned(),
                opacity: 1.0,
                sets: Vec::new(),
                engine: Some("sam2.1".to_owned()),
                points: vec!["0.5,0.4,+".to_owned()],
                bbox: None,
                invert: false,
            },
            &opts,
        )
        .expect_err("uninstalled sam2.1 engine override must fail");
        assert_eq!(uninst_err.code, "MODEL_NOT_INSTALLED");

        // Valid `rd layer add --kind ai-select --point 0.5,0.4,+ --point 0.1,0.1,- --box 0.2,0.2,0.8,0.8`
        assert_eq!(
            cmd_layer(
                LayerCmd::Add {
                    photo: photo.clone(),
                    name: "Smart Object".to_owned(),
                    kind: "ai-select".to_owned(),
                    opacity: 0.9,
                    sets: vec!["exposure=0.45".to_owned()],
                    engine: Some("heuristic".to_owned()),
                    points: vec!["0.5,0.4,+".to_owned(), "0.1,0.1,-".to_owned()],
                    bbox: Some("0.2,0.2,0.8,0.8".to_owned()),
                    invert: false,
                },
                &opts,
            )
            .expect("layer add ai-select"),
            0
        );

        // Valid `rd layer mask-add --kind ai-select --op subtract --point 0.5,0.5 --invert`
        assert_eq!(
            cmd_layer(
                LayerCmd::MaskAdd {
                    photo: photo.clone(),
                    index: "0".to_owned(),
                    kind: "ai-select".to_owned(),
                    op: "subtract".to_owned(),
                    start: None,
                    end: None,
                    center: None,
                    radius: None,
                    angle: 0.0,
                    feather: None,
                    low: 0.0,
                    high: 1.0,
                    low_falloff: 0.1,
                    high_falloff: 0.1,
                    inverted: false,
                    engine: Some("heuristic".to_owned()),
                    points: vec!["0.5,0.5".to_owned()],
                    bbox: None,
                    invert: true,
                },
                &opts,
            )
            .expect("layer mask-add ai-select"),
            0
        );

        let meta = PhotoMeta::load_sidecar(&photo).unwrap().unwrap();
        assert_eq!(meta.recipe.layers.len(), 1);
        assert_eq!(meta.recipe.layers[0].masks.len(), 2);
        assert_eq!(meta.recipe.layers[0].masks[1].op, MaskCombineOp::Subtract);
        match &meta.recipe.layers[0].masks[0].shape {
            MaskShape::RasterMask { width, height, .. } => {
                assert_eq!(*width, LAYER_MASK_TILE_SIZE as u32);
                assert_eq!(*height, LAYER_MASK_TILE_SIZE as u32);
            }
            other => panic!("expected 512x512 RasterMask, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn batch_run_blocks_traversal_neighbour_overwrite_and_existing_files_without_overwrite() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "rune_cli_batch_safety_{}_{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let photo_a = dir.join("IMG_0001.jpg");
        let photo_b = dir.join("IMG_0001.png");
        let img = rune_editor_decode::generate_demo_hdr_scene(64, 48);
        rune_editor_decode::export_image(
            &img,
            &DevelopRecipe::default(),
            &photo_a,
            ExportFormat::Jpeg { quality: 85 },
        )
        .unwrap();
        rune_editor_decode::export_image(
            &img,
            &DevelopRecipe::default(),
            &photo_b,
            ExportFormat::Png8,
        )
        .unwrap();

        let opts = RuntimeOpts {
            format: ResolvedFormat::Json,
            dry_run: false,
            quiet: true,
            verbose: false,
            backend: crate::BackendMode::Cpu,
        };

        // 1. Path traversal (`../`) in filename_template must be rejected.
        let traversal_manifest = dir.join("job_traversal.json");
        std::fs::write(
            &traversal_manifest,
            serde_json::to_string(&json!({
                "schema_version": 1,
                "inputs": { "paths": [photo_a] },
                "exports": [{
                    "id": "bad",
                    "output_dir": dir.join("out"),
                    "filename_template": "../escape.{ext}",
                    "format": "jpeg"
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(cmd_batch_run(traversal_manifest.to_str().unwrap(), 1, &opts).is_err());

        // 2. Exporting a PNG pair into the same folder as `{stem}.jpg` must not clobber neighbouring `IMG_0001.jpg`.
        let neighbour_manifest = dir.join("job_neighbour.json");
        std::fs::write(
            &neighbour_manifest,
            serde_json::to_string(&json!({
                "schema_version": 1,
                "inputs": { "paths": [&dir] },
                "exports": [{
                    "id": "clobber",
                    "output_dir": &dir,
                    "filename_template": "{stem}.{ext}",
                    "format": "jpeg",
                    "overwrite": true
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(cmd_batch_run(neighbour_manifest.to_str().unwrap(), 1, &opts).is_err());

        // 3. Existing destination file without `overwrite: true` must fail cleanly.
        let out_dir = dir.join("exports");
        std::fs::create_dir_all(&out_dir).unwrap();
        let existing = out_dir.join("IMG_0001.jpg");
        std::fs::write(&existing, b"keep-me").unwrap();
        let existing_manifest = dir.join("job_exists.json");
        std::fs::write(
            &existing_manifest,
            serde_json::to_string(&json!({
                "schema_version": 1,
                "inputs": { "paths": [&photo_a] },
                "exports": [{
                    "id": "no_overwrite",
                    "output_dir": &out_dir,
                    "filename_template": "{stem}.{ext}",
                    "format": "jpeg"
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(cmd_batch_run(existing_manifest.to_str().unwrap(), 1, &opts).is_err());
        assert_eq!(std::fs::read(&existing).unwrap(), b"keep-me");

        // 4. `rd preview` refuses to overwrite an existing file without `--overwrite`.
        assert!(
            cmd_preview(
                PreviewArgs {
                    photo: photo_a.clone(),
                    out: existing.clone(),
                    max_edge: 128,
                    quality: 80,
                    no_crop: false,
                    overwrite: false,
                },
                &opts,
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&existing).unwrap(), b"keep-me");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
