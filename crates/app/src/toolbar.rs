//! Unified Apple macOS Tahoe HIG titlebar & Anomaly OpenCode top toolbar.
//!
//! Respects `Cx::chrome_insets()` so macOS traffic lights never collide with titlebar
//! controls, enables native `.window_drag()` + double-click-to-zoom on empty bar area,
//! and isolates interactive control clusters with `.no_window_drag()`.

use rune::{ChromeInsets, El, icon, row, text};
use rune_kit::{
    Badge, BadgeVariant, Button, ButtonVariant, ControlSize, Divider, IconButton,
    IconButtonVariant, Kit, SegmentedControl,
};
use rune_theme_opencode::icons;

use crate::menu::{trigger_export_dialog, trigger_open_dialog, trigger_open_folder};
use crate::state::AppState;

pub const TITLEBAR_HEIGHT: f32 = 48.0;

pub fn top_toolbar(s: &AppState, k: &Kit, insets: ChromeInsets) -> El {
    let left_pad = insets.leading + 14.0;
    let right_pad = insets.trailing + 14.0;
    let band_h = insets.height.max(TITLEBAR_HEIGHT);

    // Left cluster: monochrome brand glyph + title + document filename + badges
    let mut left = row()
        .key("toolbar-left")
        .items_center()
        .gap(8.0)
        .min_w(0.0)
        .shrink()
        .no_window_drag()
        .child(
            row()
                .items_center()
                .gap(6.0)
                .child(icon(&icons::PHOTO, 15.0).color(k.c.icon_base).a11y_hidden())
                .child(
                    text("Rune Darkroom")
                        .font(k.ty.ui_medium)
                        .color(k.c.text_base),
                ),
        )
        .child(el_v_divider(k))
        .child(
            text(s.image.display_name.as_str())
                .font(k.ty.text_xsmall)
                .color(k.c.text_muted)
                .truncate(),
        );

    if s.is_dirty() {
        left = left.child(Badge::new("Edited").variant(BadgeVariant::Accent).build(k));
    } else {
        left = left.child(Badge::new("16-BIT").variant(BadgeVariant::Subtle).build(k));
    }

    // Center cluster: OpenCode SegmentedControl for Zoom + Before/After Split toggle
    let zoom_sel = if (s.camera.zoom - 1.0).abs() < 0.03 {
        Some(0)
    } else if (s.camera.zoom - 1.5).abs() < 0.15 {
        Some(1)
    } else if (s.camera.zoom - 2.0).abs() < 0.15 {
        Some(2)
    } else {
        None
    };

    let zoom_seg = SegmentedControl::new(&["Fit", "150%", "200%"], zoom_sel)
        .label("Viewport zoom")
        .fit()
        .shortcut(0, "cmd+0")
        .shortcut(1, "cmd+1")
        .shortcut(2, "cmd+2")
        .on_pick(|s: &mut AppState, i, _| match i {
            0 => s.camera.reset(),
            1 => s.camera.zoom = 1.5,
            _ => s.camera.zoom = 2.0,
        })
        .build(k);

    let split_btn = Button::new("Compare")
        .variant(if s.camera.split_compare {
            ButtonVariant::Contrast
        } else {
            ButtonVariant::Neutral
        })
        .control_size(ControlSize::Regular)
        .icon(&icons::SPLIT)
        .help("Toggle Before / After split compare")
        .shortcut("\\")
        .on_press_tx(|s: &mut AppState, tx| {
            s.camera.split_compare = !s.camera.split_compare;
            s.sync_window_state(tx);
        })
        .build(k);

    let center = row()
        .key("toolbar-center")
        .items_center()
        .gap(8.0)
        .no_window_drag()
        .child(zoom_seg)
        .child(split_btn);

    // Right cluster: Open / Folder / Undo / Redo / Filmstrip / Inspector / Export CTA
    let open_photo = IconButton::new(&icons::OPEN_FILE)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Regular)
        .label("Open Photo")
        .help("Open a RAW or image file")
        .shortcut("cmd+o")
        .on_press_tx(|_: &mut AppState, tx| trigger_open_dialog(tx))
        .build(k);

    let open_folder = IconButton::new(&icons::FOLDER)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Regular)
        .label("Open Folder")
        .help("Open a folder of photos into the filmstrip")
        .shortcut("cmd+shift+o")
        .on_press_tx(|_: &mut AppState, tx| trigger_open_folder(tx))
        .build(k);

    let undo_btn = IconButton::new(&icons::OUTLINE_UNDO)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Regular)
        .label("Undo")
        .help("Undo last adjustment")
        .shortcut("cmd+z")
        .disabled(!s.history.can_undo())
        .on_press_tx(|s: &mut AppState, tx| s.undo_tx(tx))
        .build(k);

    let redo_btn = IconButton::new(&icons::OUTLINE_ROTATE_CLOCKWISE)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Regular)
        .label("Redo")
        .help("Redo adjustment")
        .shortcut("cmd+shift+z")
        .disabled(!s.history.can_redo())
        .on_press_tx(|s: &mut AppState, tx| s.redo_tx(tx))
        .build(k);

    let filmstrip_icon = if s.show_filmstrip {
        &icons::LAYOUT_BOTTOM_FULL
    } else {
        &icons::LAYOUT_BOTTOM
    };
    let filmstrip_btn = IconButton::new(filmstrip_icon)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Regular)
        .label("Toggle Filmstrip")
        .help("Show or hide bottom filmstrip")
        .shortcut("F")
        .expanded(s.show_filmstrip)
        .on_press_tx(|s: &mut AppState, tx| {
            s.show_filmstrip = !s.show_filmstrip;
            s.sync_window_state(tx);
        })
        .build(k);

    let inspector_icon = if s.show_inspector {
        &icons::LAYOUT_RIGHT_FULL
    } else {
        &icons::LAYOUT_RIGHT
    };
    let inspector_btn = IconButton::new(inspector_icon)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Regular)
        .label("Toggle Inspector")
        .help("Show or hide develop inspector")
        .shortcut("I")
        .expanded(s.show_inspector)
        .on_press_tx(|s: &mut AppState, tx| {
            s.show_inspector = !s.show_inspector;
            s.sync_window_state(tx);
        })
        .build(k);

    let export_btn = Button::new("Export")
        .variant(ButtonVariant::Contrast)
        .control_size(ControlSize::Regular)
        .icon(&icons::SHARE)
        .help("Export full-resolution developed photo")
        .shortcut("cmd+shift+e")
        .on_press_tx(|s: &mut AppState, tx| trigger_export_dialog(s, tx))
        .build(k);

    let right = row()
        .key("toolbar-right")
        .items_center()
        .gap(4.0)
        .no_window_drag()
        .child(open_photo)
        .child(open_folder)
        .child(el_v_divider(k))
        .child(undo_btn)
        .child(redo_btn)
        .child(el_v_divider(k))
        .child(filmstrip_btn)
        .child(inspector_btn)
        .child(el_v_divider(k))
        .child(export_btn);

    row()
        .key("toolbar")
        .role("toolbar")
        .label("Darkroom toolbar")
        .w_full()
        .h(band_h)
        .pl(left_pad)
        .pr(right_pad)
        .gap(12.0)
        .center_y()
        .justify_between()
        .fill(k.c.bg_deep)
        .border_sides([0.0, 0.0, 1.0, 0.0], k.c.border_base)
        .window_drag()
        .child(left)
        .child(center)
        .child(right)
}

fn el_v_divider(k: &Kit) -> El {
    row()
        .px(2.0)
        .h(16.0)
        .items_center()
        .child(Divider::vertical().build(k))
}
