//! Native macOS Menu Bar (`rune::MenuItem`) following Apple macOS HIG menu ordering
//! and keyboard shortcuts, with live enabled/checked synchronization.

use rune::file_dialog::FileDialogOptions;
use rune::{MenuItem, Shortcut, Tx};
use rune_editor_decode::SUPPORTED_EXTENSIONS;

use crate::state::AppState;

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

/// Builds the full native macOS menu bar snapshot (App, File, Edit, View).
/// Standard Window menu items (`Close ⌘W`) are installed by `App::standard_menus()`.
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
    let proof_margin_active = s.proof_margin != crate::state::ProofMargin::Compact;
    let proof_margin_label = format!("Proof Margin: {}", s.proof_margin.label());

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
        // File menu
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
        MenuItem::new("New Capture One Session…", |_: &mut AppState, tx| {
            trigger_new_session(tx)
        })
        .id("file-new-session")
        .menu("File"),
        MenuItem::new("Watch Hot Folder…", |_: &mut AppState, tx| {
            trigger_watch_hot_folder(tx)
        })
        .id("file-watch-hot-folder")
        .menu("File"),
        MenuItem::separator().menu("File"),
        MenuItem::new("Move to Selects", |s: &mut AppState, tx| {
            s.move_active_to_selects_tx(tx)
        })
        .id("file-move-selects")
        .menu("File")
        .key_equivalent(sc("cmd+j"))
        .enabled(has_disk_photo),
        MenuItem::new("Edit With External Editor…", |s: &mut AppState, tx| {
            s.edit_with_external_editor_tx(tx)
        })
        .id("file-edit-external")
        .menu("File")
        .key_equivalent(sc("cmd+shift+w"))
        .enabled(has_disk_photo),
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
        // Edit menu
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
        MenuItem::new("Paste Selected Adjustments", |s: &mut AppState, tx| {
            s.paste_selected_adjustments_tx(tx)
        })
        .id("edit-paste-adj")
        .menu("Edit")
        .after("edit-copy-adj")
        .key_equivalent(sc("cmd+shift+v"))
        .enabled(has_clipboard),
        MenuItem::separator().menu("Edit"),
        MenuItem::new("New Variant", |s: &mut AppState, tx| {
            s.new_variant_tx(None, tx)
        })
        .id("edit-new-variant")
        .menu("Edit"),
        MenuItem::new("Clone Variant", |s: &mut AppState, tx| {
            s.clone_variant_tx(None, tx)
        })
        .id("edit-clone-variant")
        .menu("Edit"),
        MenuItem::new("Next Variant", |s: &mut AppState, tx| s.next_variant_tx(tx))
            .id("edit-next-variant")
            .menu("Edit")
            .enabled(can_del_variant),
        MenuItem::new("Delete Active Variant", |s: &mut AppState, tx| {
            s.delete_active_variant_tx(tx)
        })
        .id("edit-delete-variant")
        .menu("Edit")
        .enabled(can_del_variant),
        MenuItem::new("Compare Variant", |s: &mut AppState, tx| {
            s.toggle_compare_variant_tx(tx)
        })
        .id("edit-compare-variant")
        .menu("Edit")
        .key_equivalent(sc("cmd+enter"))
        .checked(has_compare),
        MenuItem::separator().menu("Edit"),
        MenuItem::new("Reset Adjustments", |s: &mut AppState, tx| {
            s.reset_recipe_tx(tx)
        })
        .id("edit-reset")
        .menu("Edit")
        .key_equivalent(sc("cmd+r")),
        // View menu
        MenuItem::new("Zoom to Fit", |s: &mut AppState, _| s.camera.reset())
            .id("view-zoom-fit")
            .menu("View")
            .key_equivalent(sc("cmd+0")),
        MenuItem::new("Zoom 150%", |s: &mut AppState, _| {
            s.camera.zoom = 1.5;
            s.camera.pan = [0.0, 0.0];
        })
        .id("view-zoom-150")
        .menu("View")
        .key_equivalent(sc("cmd+1")),
        MenuItem::new("Zoom 200%", |s: &mut AppState, _| {
            s.camera.zoom = 2.0;
        })
        .id("view-zoom-200")
        .menu("View")
        .key_equivalent(sc("cmd+2")),
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
        MenuItem::new("Exposure Clipping Warning", |s: &mut AppState, tx| {
            s.toggle_exposure_warning_tx(tx)
        })
        .id("view-exposure-warning")
        .menu("View")
        .key_equivalent(sc("cmd+e"))
        .keys_first()
        .checked(exposure_warning),
        MenuItem::new("Focus Mask", |s: &mut AppState, tx| {
            s.toggle_focus_mask_tx(tx)
        })
        .id("view-focus-mask")
        .menu("View")
        .key_equivalent(sc("cmd+m"))
        .keys_first()
        .checked(focus_mask),
        MenuItem::new("Red Mask Overlay", |s: &mut AppState, tx| {
            s.toggle_red_mask_overlay_tx(tx)
        })
        .id("view-mask-red")
        .menu("View")
        .key_equivalent(sc("m"))
        .keys_first()
        .checked(mask_red),
        MenuItem::new("Grayscale Mask Overlay", |s: &mut AppState, tx| {
            s.toggle_gray_mask_overlay_tx(tx)
        })
        .id("view-mask-gray")
        .menu("View")
        .key_equivalent(sc("alt+m"))
        .keys_first()
        .checked(mask_gray),
        MenuItem::new("Crop & Geometry Mode", |s: &mut AppState, tx| {
            s.toggle_crop_mode_tx(tx)
        })
        .id("view-crop-mode")
        .menu("View")
        .key_equivalent(sc("c"))
        .keys_first()
        .checked(crop_mode),
        MenuItem::new("GPU Loupe Magnifier", |s: &mut AppState, tx| {
            s.toggle_loupe_tx(tx)
        })
        .id("view-loupe")
        .menu("View")
        .key_equivalent(sc("alt+l"))
        .keys_first()
        .checked(loupe_on),
        MenuItem::new("Grid Browser", |s: &mut AppState, tx| {
            s.toggle_grid_browser_tx(tx)
        })
        .id("view-grid-browser")
        .menu("View")
        .key_equivalent(sc("g"))
        .keys_first()
        .checked(grid_on),
        MenuItem::new("Auto-Advance on Cull", |s: &mut AppState, tx| {
            s.auto_advance = !s.auto_advance;
            s.sync_window_state(tx);
        })
        .id("view-auto-advance")
        .menu("View")
        .checked(auto_adv),
        MenuItem::new(proof_margin_label, |s: &mut AppState, tx| {
            s.cycle_proof_margin_tx(tx)
        })
        .id("view-proof-margin")
        .menu("View")
        .key_equivalent(sc("cmd+shift+m"))
        .checked(proof_margin_active),
        MenuItem::separator().menu("View"),
        MenuItem::new("Toggle Filmstrip", |s: &mut AppState, tx| {
            s.show_filmstrip = !s.show_filmstrip;
            s.sync_window_state(tx);
        })
        .id("view-filmstrip")
        .menu("View")
        .key_equivalent(sc("cmd+alt+f"))
        .checked(show_filmstrip),
        MenuItem::new("Toggle Inspector", |s: &mut AppState, tx| {
            s.show_inspector = !s.show_inspector;
            s.sync_window_state(tx);
        })
        .id("view-inspector")
        .menu("View")
        .key_equivalent(sc("cmd+alt+i"))
        .checked(show_inspector),
    ]
}
