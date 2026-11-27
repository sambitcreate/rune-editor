//! Native macOS Menu Bar (`rune::MenuItem`) following Apple macOS HIG 5-menu ordering
//! (`File`, `Edit`, `Image`, `View`, `Window`) with ≤8 scannable top-level items per menu,
//! clean submenus, and live enabled/checked synchronization.

use rune::file_dialog::FileDialogOptions;
use rune::{MenuItem, Shortcut, Tx};
use rune_editor_core::{ALLOWED_PROFILE_EXTENSIONS, ColorTag};
use rune_editor_decode::SUPPORTED_EXTENSIONS;

use crate::state::{AppState, ProofMargin};

fn sc(s: &str) -> Shortcut {
    s.parse().expect("valid menu shortcut")
}

pub fn trigger_open_dialog(tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Open Photo or RAW".into(),
        ..Default::default()
    }
    .filter("Camera RAW & Photos", SUPPORTED_EXTENSIONS);

    tx.open_file(options, |s: &mut AppState, result, tx| match result {
        Ok(Some(path)) => s.open_image_path_tx(path, tx),
        Ok(None) => {}
        Err(err) => s.status_message = Some(format!("Open dialog error: {err}")),
    });
}

pub fn trigger_import_profile_dialog(tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Import Camera Profile (.dcp, .icc, .cube)".into(),
        ..Default::default()
    }
    .filter(
        "Camera Profiles (.dcp, .icc, .cube)",
        ALLOWED_PROFILE_EXTENSIONS,
    );

    tx.open_file(options, |s: &mut AppState, result, tx| match result {
        Ok(Some(path)) => s.import_camera_profile_path_tx(path, tx),
        Ok(None) => {}
        Err(err) => s.status_message = Some(format!("Profile import dialog error: {err}")),
    });
}

pub fn trigger_open_folder(tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Open Photo Folder".into(),
        ..Default::default()
    };

    tx.open_folder(options, |s: &mut AppState, result, tx| match result {
        Ok(Some(dir)) => s.open_folder_tx(dir, tx),
        Ok(None) => {}
        Err(err) => s.status_message = Some(format!("Folder dialog error: {err}")),
    });
}

pub fn trigger_new_session(tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Select Root Folder for New Capture One Session".into(),
        ..Default::default()
    };

    tx.open_folder(options, |s: &mut AppState, result, tx| match result {
        Ok(Some(dir)) => s.create_capture_one_session_tx(dir, tx),
        Ok(None) => {}
        Err(err) => s.status_message = Some(format!("Session dialog error: {err}")),
    });
}

pub fn trigger_watch_hot_folder(tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Select Hot Folder to Watch for New Shots".into(),
        ..Default::default()
    };

    tx.open_folder(options, |s: &mut AppState, result, tx| match result {
        Ok(Some(dir)) => s.start_hot_folder_watch_tx(dir, tx),
        Ok(None) => {}
        Err(err) => s.status_message = Some(format!("Hot Folder dialog error: {err}")),
    });
}

pub fn trigger_export_dialog(s: &AppState, tx: &mut Tx) {
    let stem = s
        .image
        .source_path
        .as_deref()
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Editorial_Portrait".to_owned());

    let options = FileDialogOptions {
        title: "Export Developed Photo".into(),
        file_name: Some(format!("{stem}_developed.jpg")),
        ..Default::default()
    }
    .filter("JPEG Image", &["jpg", "jpeg"])
    .filter("WebP Image", &["webp"])
    .filter("16-bit PNG", &["png"])
    .filter("16-bit TIFF", &["tiff", "tif"]);

    tx.save_file(options, |s: &mut AppState, result, tx| match result {
        Ok(Some(dest)) => s.export_to_path_tx(dest, tx),
        Ok(None) => {}
        Err(err) => s.status_message = Some(format!("Export dialog error: {err}")),
    });
}

/// Builds the full native macOS 5-menu snapshot (`App`, `File`, `Edit`, `Image`, `View`, `Window`).
/// Standard Window items (`Close ⌘W`, Minimize, Zoom) are installed by `App::standard_menus()`.
pub fn build_menu_items(s: &AppState) -> Vec<MenuItem<AppState>> {
    let can_undo = s.history.can_undo();
    let can_redo = s.history.can_redo();
    let has_clipboard = s.catalog.has_clipboard();
    let is_split = s.camera.split_compare;
    let exposure_warning = s.camera.exposure_warning;
    let focus_mask = s.camera.focus_mask;
    let crop_mode = s.camera.crop_mode;
    let mask_red = s.camera.mask_overlay_mode == 1;
    let mask_gray = s.camera.mask_overlay_mode == 2;
    let loupe_on = s.camera.loupe_enabled;
    let grid_on = s.show_grid_browser;
    let auto_adv = s.auto_advance;
    let has_compare = s.compare_reference.is_some();
    let can_del_variant = s.active_variant_count() > 1;
    let show_filmstrip = s.show_filmstrip;
    let show_inspector = s.show_inspector;
    let has_disk_photo = !s.demo_mode && s.image.source_path.is_some();
    let active_tag = s
        .catalog
        .active_entry()
        .map_or(ColorTag::None, |e| e.effective_color_tag());
    let proof_margin = s.proof_margin;

    // Color Tag submenu items (⌥0..⌥7)
    let color_tag_items: Vec<MenuItem<AppState>> = ColorTag::ALL
        .into_iter()
        .enumerate()
        .map(|(i, tag)| {
            let chord = format!("alt+{i}");
            MenuItem::with_action(tag.label(), move |s: &mut AppState, tx| {
                s.set_active_color_tag_tx(tag, tx);
            })
            .id(format!("image-tag-{}", tag.cli_slug()))
            .key_equivalent(sc(&chord))
            .keys_first()
            .checked(active_tag == tag)
        })
        .collect();

    vec![
        // App menu
        MenuItem::about("About Rune Darkroom", |s: &mut AppState, _| {
            s.show_about = true;
        })
        .id("app-about"),
        MenuItem::new("Settings & Local MCP…", |s: &mut AppState, tx| {
            let next = !s.show_settings;
            s.set_show_settings_tx(next, tx);
        })
        .id("app-settings")
        .key_equivalent(sc("cmd+,")),
        // 1. File menu (≤8 top-level items)
        MenuItem::new("Open Photo…", |_: &mut AppState, tx| {
            trigger_open_dialog(tx)
        })
        .id("file-open")
        .menu("File")
        .key_equivalent(sc("cmd+o")),
        MenuItem::new("Open Folder…", |_: &mut AppState, tx| {
            trigger_open_folder(tx)
        })
        .id("file-open-folder")
        .menu("File")
        .key_equivalent(sc("cmd+shift+o")),
        MenuItem::submenu(
            "Session",
            [
                MenuItem::new("New 4-Folder Session…", |_: &mut AppState, tx| {
                    trigger_new_session(tx)
                })
                .id("file-new-session"),
                MenuItem::new("Watch Hot Folder…", |_: &mut AppState, tx| {
                    trigger_watch_hot_folder(tx)
                })
                .id("file-watch-hot-folder"),
                MenuItem::separator(),
                MenuItem::new("Move to Selects", |s: &mut AppState, tx| {
                    s.move_active_to_selects_tx(tx)
                })
                .id("file-move-selects")
                .key_equivalent(sc("cmd+j"))
                .enabled(has_disk_photo),
            ],
        )
        .id("file-session-submenu")
        .menu("File"),
        MenuItem::separator().menu("File"),
        MenuItem::new("Save Sidecar", |s: &mut AppState, tx| s.save_sidecar_tx(tx))
            .id("file-save")
            .menu("File")
            .key_equivalent(sc("cmd+s")),
        MenuItem::new("Export Recipes…", |s: &mut AppState, tx| {
            let next = !s.show_export;
            s.set_show_export_tx(next, tx);
        })
        .id("file-export")
        .menu("File")
        .key_equivalent(sc("cmd+shift+e")),
        MenuItem::new("Quick Single Export…", |s: &mut AppState, tx| {
            trigger_export_dialog(s, tx)
        })
        .id("file-export-single")
        .menu("File")
        .key_equivalent(sc("cmd+alt+e")),
        MenuItem::separator().menu("File"),
        MenuItem::new("Edit With External Editor…", |s: &mut AppState, tx| {
            s.edit_with_external_editor_tx(tx)
        })
        .id("file-edit-external")
        .menu("File")
        .key_equivalent(sc("cmd+shift+w"))
        .enabled(has_disk_photo),
        // 2. Edit menu (5 items)
        MenuItem::new("Undo", |s: &mut AppState, tx| s.undo_tx(tx))
            .id("edit-undo")
            .menu("Edit")
            .key_equivalent(sc("cmd+z"))
            .keys_first()
            .enabled(can_undo),
        MenuItem::new("Redo", |s: &mut AppState, tx| s.redo_tx(tx))
            .id("edit-redo")
            .menu("Edit")
            .key_equivalent(sc("cmd+shift+z"))
            .keys_first()
            .enabled(can_redo),
        MenuItem::separator().menu("Edit").after("selectAll:"),
        MenuItem::new("Copy Adjustments", |s: &mut AppState, tx| {
            s.copy_adjustments_tx(tx)
        })
        .id("edit-copy-adj")
        .menu("Edit")
        .after("selectAll:")
        .key_equivalent(sc("cmd+shift+c")),
        MenuItem::new("Apply Adjustments", |s: &mut AppState, tx| {
            s.paste_selected_adjustments_tx(tx)
        })
        .id("edit-paste-adj")
        .menu("Edit")
        .after("edit-copy-adj")
        .key_equivalent(sc("cmd+shift+v"))
        .enabled(has_clipboard),
        MenuItem::separator().menu("Edit"),
        MenuItem::new("Reset All", |s: &mut AppState, tx| s.reset_recipe_tx(tx))
            .id("edit-reset")
            .menu("Edit")
            .key_equivalent(sc("cmd+r")),
        // 3. Image menu (NEW — 4 scannable top-level items with 3 submenus)
        MenuItem::submenu(
            "Variants",
            [
                MenuItem::new("New Variant", |s: &mut AppState, tx| {
                    s.new_variant_tx(None, tx)
                })
                .id("image-new-variant"),
                MenuItem::new("Clone Variant", |s: &mut AppState, tx| {
                    s.clone_variant_tx(None, tx)
                })
                .id("image-clone-variant"),
                MenuItem::new("Next Variant", |s: &mut AppState, tx| s.next_variant_tx(tx))
                    .id("image-next-variant")
                    .enabled(can_del_variant),
                MenuItem::new("Compare Variant", |s: &mut AppState, tx| {
                    s.toggle_compare_variant_tx(tx)
                })
                .id("image-compare-variant")
                .key_equivalent(sc("cmd+enter"))
                .checked(has_compare),
                MenuItem::separator(),
                MenuItem::new("Delete Active Variant", |s: &mut AppState, tx| {
                    s.delete_active_variant_tx(tx)
                })
                .id("image-delete-variant")
                .enabled(can_del_variant),
            ],
        )
        .id("image-variants-submenu")
        .menu("Image"),
        MenuItem::submenu(
            "Auto Adjust",
            [
                MenuItem::new("Auto White Balance", |s: &mut AppState, tx| {
                    s.auto_white_balance_tx(tx)
                })
                .id("image-auto-wb"),
                MenuItem::new("Auto Tone", |s: &mut AppState, tx| s.auto_tone_tx(tx))
                    .id("image-auto-tone"),
                MenuItem::new("Auto Levels", |s: &mut AppState, tx| s.auto_levels_tx(tx))
                    .id("image-auto-levels"),
            ],
        )
        .id("image-auto-submenu")
        .menu("Image"),
        MenuItem::submenu("Color Tag", color_tag_items)
            .id("image-color-tag-submenu")
            .menu("Image"),
        MenuItem::separator().menu("Image"),
        MenuItem::new("Auto-Advance on Cull", |s: &mut AppState, tx| {
            s.auto_advance = !s.auto_advance;
            s.sync_window_state(tx);
        })
        .id("image-auto-advance")
        .menu("Image")
        .checked(auto_adv),
        // 4. View menu (7 top-level items with 3 submenus & unified shortcuts)
        MenuItem::submenu(
            "Zoom",
            [
                MenuItem::new("Zoom to Fit", |s: &mut AppState, _| s.camera.reset())
                    .id("view-zoom-fit")
                    .key_equivalent(sc("cmd+0")),
                MenuItem::new("Zoom 150%", |s: &mut AppState, _| {
                    s.camera.zoom = 1.5;
                    s.camera.pan = [0.0, 0.0];
                })
                .id("view-zoom-150")
                .key_equivalent(sc("cmd+1")),
                MenuItem::new("Zoom 200%", |s: &mut AppState, _| {
                    s.camera.zoom = 2.0;
                })
                .id("view-zoom-200")
                .key_equivalent(sc("cmd+2")),
            ],
        )
        .id("view-zoom-submenu")
        .menu("View"),
        MenuItem::submenu(
            "Overlays & Diagnostics",
            [
                MenuItem::new("Exposure Clipping Warning", |s: &mut AppState, tx| {
                    s.toggle_exposure_warning_tx(tx)
                })
                .id("view-exposure-warning")
                .key_equivalent(sc("cmd+e"))
                .keys_first()
                .checked(exposure_warning),
                MenuItem::new("Focus Mask", |s: &mut AppState, tx| {
                    s.toggle_focus_mask_tx(tx)
                })
                .id("view-focus-mask")
                .key_equivalent(sc("cmd+m"))
                .keys_first()
                .checked(focus_mask),
                MenuItem::separator(),
                MenuItem::new("Red Mask Overlay", |s: &mut AppState, tx| {
                    s.toggle_red_mask_overlay_tx(tx)
                })
                .id("view-mask-red")
                .key_equivalent(sc("m"))
                .keys_first()
                .checked(mask_red),
                MenuItem::new("Grayscale Mask Overlay", |s: &mut AppState, tx| {
                    s.toggle_gray_mask_overlay_tx(tx)
                })
                .id("view-mask-gray")
                .key_equivalent(sc("alt+m"))
                .keys_first()
                .checked(mask_gray),
                MenuItem::separator(),
                MenuItem::new("GPU Loupe Magnifier", |s: &mut AppState, tx| {
                    s.toggle_loupe_tx(tx)
                })
                .id("view-loupe")
                .key_equivalent(sc("o"))
                .keys_first()
                .checked(loupe_on),
                MenuItem::new("Crop & Geometry Mode", |s: &mut AppState, tx| {
                    s.toggle_crop_mode_tx(tx)
                })
                .id("view-crop-mode")
                .key_equivalent(sc("c"))
                .keys_first()
                .checked(crop_mode),
            ],
        )
        .id("view-overlays-submenu")
        .menu("View"),
        MenuItem::submenu(
            "Proof Margin",
            [
                MenuItem::new("Compact (16 pt)", |s: &mut AppState, tx| {
                    s.set_proof_margin_tx(ProofMargin::Compact, tx)
                })
                .id("view-margin-compact")
                .checked(proof_margin == ProofMargin::Compact),
                MenuItem::new("Studio (48 pt)", |s: &mut AppState, tx| {
                    s.set_proof_margin_tx(ProofMargin::Studio, tx)
                })
                .id("view-margin-studio")
                .checked(proof_margin == ProofMargin::Studio),
                MenuItem::new("Exhibition (84 pt)", |s: &mut AppState, tx| {
                    s.set_proof_margin_tx(ProofMargin::Exhibition, tx)
                })
                .id("view-margin-exhibition")
                .checked(proof_margin == ProofMargin::Exhibition),
                MenuItem::separator(),
                MenuItem::new("Cycle Proof Margin", |s: &mut AppState, tx| {
                    s.cycle_proof_margin_tx(tx)
                })
                .id("view-proof-margin")
                .key_equivalent(sc("cmd+shift+m")),
            ],
        )
        .id("view-proof-margin-submenu")
        .menu("View"),
        MenuItem::separator().menu("View"),
        MenuItem::new("Before / After Split", |s: &mut AppState, tx| {
            s.camera.split_compare = !s.camera.split_compare;
            s.sync_window_state(tx);
        })
        .id("view-compare")
        .menu("View")
        .key_equivalent(sc("backslash"))
        .keys_first()
        .checked(is_split),
        MenuItem::new("Grid Browser", |s: &mut AppState, tx| {
            s.toggle_grid_browser_tx(tx)
        })
        .id("view-grid-browser")
        .menu("View")
        .key_equivalent(sc("g"))
        .keys_first()
        .checked(grid_on),
        MenuItem::separator().menu("View"),
        MenuItem::new("Show Filmstrip", |s: &mut AppState, tx| {
            s.show_filmstrip = !s.show_filmstrip;
            s.sync_window_state(tx);
        })
        .id("view-filmstrip")
        .menu("View")
        .key_equivalent(sc("f"))
        .keys_first()
        .checked(show_filmstrip),
        MenuItem::new("Show Inspector", |s: &mut AppState, tx| {
            s.show_inspector = !s.show_inspector;
            s.sync_window_state(tx);
        })
        .id("view-inspector")
        .menu("View")
        .key_equivalent(sc("i"))
        .keys_first()
        .checked(show_inspector),
        // 5. Window menu (augments standard macOS Window menu with Settings access)
        MenuItem::separator().menu("Window"),
        MenuItem::new("Settings & Local MCP…", |s: &mut AppState, tx| {
            let next = !s.show_settings;
            s.set_show_settings_tx(next, tx);
        })
        .id("window-settings")
        .menu("Window")
        .checked(s.show_settings),
    ]
}
