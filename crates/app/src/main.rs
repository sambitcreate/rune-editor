//! Rune Darkroom — Anomaly OpenCode-styled, Apple macOS HIG-compliant GPU-accelerated
//! Capture One / Lightroom darkroom built on Rune.
//!
//! Run interactively:
//!   cargo run -p rune-editor --release
//!
//! Render headless verification snapshots:
//!   ./target/release/rune-editor --chrome --snap darkroom.png --scene editorial
//!   ./target/release/rune-editor --chrome --snap crop.png --scene crop
//!   ./target/release/rune-editor --chrome --snap levels.png --scene levels
//!   ./target/release/rune-editor --chrome --snap bw.png --scene bw
//!   ./target/release/rune-editor --chrome --snap warnings.png --scene warnings

mod color_wheel_tex;
mod compositor;
mod filmstrip;
mod menu;
mod state;
mod tokens;
mod toolbar;
mod widgets;

use rune::{
    App, AppSpec, CloseResponse, Color, ColorScheme, Cursor, Cx, Drag, El, Key, KeyEvent,
    MacColorSpace, MacWindowOptions, MagnifyEvent, Phase, PointerEvent, PrewarmEffects,
    RotateEvent, Titlebar, Tx, WheelEvent, col, effect, el, row, text,
};
use rune_editor_core::{
    ColorWheel, CompositionGrid, CropAspect, CullFlag, HslBand, ToneCurves, auto_levels,
};
use rune_kit::{
    Button, ButtonVariant, ControlSize, Divider, Keybind, KeybindVariant, Kit, scrim_tx,
};

use crate::compositor::DarkroomCompositor;
use crate::filmstrip::filmstrip_bar;
use crate::menu::{
    build_menu_items, trigger_export_dialog, trigger_open_dialog, trigger_open_folder,
};
use crate::state::{
    AppState, CursorColorReadout, CurveChannel, InspectorTab, PHOTO_VIEWPORT_MARK, SpeedEditParam,
};
use crate::tokens::{border, color, size, space, ty};
use crate::toolbar::top_toolbar;
use crate::widgets::right_inspector;

/// Compact translucent OpenCode pill for active viewer diagnostics (`CLIPPING`, `FOCUS MASK`, `CROP`).
fn diagnostic_hud_pill(k: &Kit, key: &'static str, label: &'static str, fg: Color) -> El {
    k.elevate(
        el().key(key)
            .px(space::MD)
            .py(space::TINY)
            .radius(k.radius.md)
            .continuous_corners()
            .backdrop_blur(size::HUD_BLUR)
            .fill(color::hud_surface(k))
            .border(border::HAIRLINE, fg)
            .child(text(label).font(ty::section_header(k)).color(fg)),
        &k.elevation.floating,
    )
}

/// Central color-accurate GPU photo viewport surrounded by opaque neutral `#121212` matte
/// (`k.c.page`), strictly obeying Apple HIG's rule against vibrancy/translucency behind
/// color-critical canvases.
fn photo_viewport(s: &AppState, k: &Kit) -> El {
    let cursor = if s.wb_eyedropper_active || s.camera.crop_mode {
        Cursor::Crosshair
    } else if s.speed_edit.is_some() {
        Cursor::ResizeEw
    } else if s.camera.zoom > 1.01 {
        Cursor::Grab
    } else {
        Cursor::Default
    };

    let gpu_slot = effect(PHOTO_VIEWPORT_MARK)
        .key("photo-gpu-slot")
        .w_full()
        .h_full()
        .cursor(cursor)
        .on_layout_tx(|s: &mut AppState, rect, tx| {
            s.viewport_rect_pts = [rect[2], rect[3]];
            s.viewport_scale = tx.scale();
            tx.live_only();
        })
        .on_pointer_move_local(|s: &mut AppState, p: PointerEvent, _| {
            s.update_cursor_readout(p.x, p.y);
        })
        .on_hover_change(|s: &mut AppState, inside, _| {
            if !inside {
                s.cursor_readout = None;
            }
        })
        .on_magnify(|s: &mut AppState, ev: MagnifyEvent, _| {
            s.camera.zoom_by((1.0 + ev.delta).clamp(0.5, 2.0));
            true
        })
        .on_rotate(|s: &mut AppState, ev: RotateEvent, _| {
            s.camera.rotation_deg = (s.camera.rotation_deg + ev.degrees).clamp(-180.0, 180.0);
            true
        })
        .on_smart_magnify(|s: &mut AppState, _, _, _| {
            if (s.camera.zoom - 1.0).abs() < 0.05 {
                s.camera.zoom = 2.0;
            } else {
                s.camera.reset();
            }
            true
        })
        .on_wheel(|s: &mut AppState, ev: WheelEvent, tx| {
            if s.speed_edit.is_some() {
                if s.scrub_speed_edit(ev.dy * 0.008) {
                    tx.instant();
                }
                return true;
            }
            if ev.mods.cmd || ev.mods.ctrl {
                let factor = (1.0 + ev.dy * 0.005).clamp(0.8, 1.25);
                s.camera.zoom_by(factor);
            } else if s.camera.zoom > 1.01 {
                s.camera.pan_by(ev.dx * 0.0025, ev.dy * 0.0025);
            } else {
                return false;
            }
            true
        })
        .on_drag(|s: &mut AppState, d: &mut Drag, tx| {
            if s.wb_eyedropper_active {
                if matches!(d.phase, Phase::Down) {
                    s.pick_wb_at_viewport_tx(d.x, d.y, tx);
                }
                return;
            }

            if s.speed_edit.is_some() {
                match d.phase {
                    Phase::Down => s.last_drag_pos = Some((d.x, d.y)),
                    Phase::Move => {
                        if let Some((lx, _)) = s.last_drag_pos {
                            let dx = d.x - lx;
                            if s.scrub_speed_edit(dx * 0.004) {
                                tx.instant();
                            }
                        }
                        s.last_drag_pos = Some((d.x, d.y));
                    }
                    Phase::Up => {
                        s.last_drag_pos = None;
                    }
                }
                return;
            }

            if s.camera.crop_mode {
                match d.phase {
                    Phase::Down => s.begin_crop_drag(d.x, d.y, d.w, d.h),
                    Phase::Move => {
                        s.update_crop_drag(d.x, d.y, d.w, d.h);
                        tx.instant();
                    }
                    Phase::Up => s.end_crop_drag_tx(tx),
                }
                return;
            }

            match d.phase {
                Phase::Down => {
                    s.last_drag_pos = Some((d.x, d.y));
                    if s.camera.split_compare && d.w > 1.0 {
                        let frac = (d.x / d.w).clamp(0.05, 0.95);
                        if (frac - s.camera.split_pos).abs() < 0.08 {
                            s.camera.split_pos = frac;
                        }
                    }
                }
                Phase::Move => {
                    if s.camera.split_compare && d.w > 1.0 && s.camera.zoom <= 1.02 {
                        s.camera.split_pos = (d.x / d.w).clamp(0.05, 0.95);
                    } else if s.camera.zoom > 1.01
                        && let Some((lx, ly)) = s.last_drag_pos
                    {
                        let dx = (d.x - lx) / d.w.max(1.0) * 2.0;
                        let dy = (d.y - ly) / d.h.max(1.0) * 2.0;
                        s.camera.pan_by(dx, dy);
                    }
                    s.last_drag_pos = Some((d.x, d.y));
                }
                Phase::Up => {
                    s.last_drag_pos = None;
                }
            }
        });

    let mut container = el()
        .key("viewport-container")
        .grow(1.0)
        .w_full()
        .min_h(0.0)
        .fill(k.c.page)
        .border(
            if s.drop_hover {
                border::DROP_TARGET
            } else {
                border::NONE
            },
            if s.drop_hover {
                k.c.border_focus
            } else {
                Color::TRANSPARENT
            },
        )
        .on_drop_hover_within(|s: &mut AppState, over, _| {
            s.drop_hover = over;
        })
        .on_drop_files(|s: &mut AppState, files, tx| {
            s.drop_hover = false;
            if let Some(first) = files.first() {
                if first.is_dir() {
                    s.open_folder_tx(first.clone(), tx);
                } else {
                    s.open_image_path_tx(first.clone(), tx);
                }
            }
        })
        .child(gpu_slot);

    if s.camera.split_compare {
        let before_pill = k.elevate(
            el().key("hud-before")
                .pin(Some(space::HUD_X), Some(space::HUD_Y), None, None)
                .px(space::LG)
                .py(space::XS)
                .radius(k.radius.md)
                .continuous_corners()
                .backdrop_blur(size::HUD_BLUR)
                .fill(color::hud_surface(k))
                .border(border::HAIRLINE, k.c.border_base)
                .child(
                    text("BEFORE")
                        .font(ty::section_header(k))
                        .color(k.c.text_muted),
                ),
            &k.elevation.floating,
        );
        let after_pill = k.elevate(
            el().key("hud-after")
                .pin(None, Some(space::HUD_Y), Some(space::HUD_X), None)
                .px(space::LG)
                .py(space::XS)
                .radius(k.radius.md)
                .continuous_corners()
                .backdrop_blur(size::HUD_BLUR)
                .fill(color::hud_surface(k))
                .border(border::HAIRLINE, k.c.border_focus)
                .child(
                    text("AFTER")
                        .font(ty::section_header(k))
                        .color(k.c.text_accent),
                ),
            &k.elevation.floating,
        );
        container = container.child(before_pill).child(after_pill);
    }

    if s.camera.exposure_warning || s.camera.focus_mask || s.camera.crop_mode {
        let mut diag_row = row()
            .key("hud-diagnostics")
            .pin(
                Some(space::HUD_X),
                Some(if s.camera.split_compare {
                    space::HUD_BELOW_SPLIT
                } else {
                    space::HUD_Y
                }),
                None,
                None,
            )
            .items_center()
            .gap(space::SM);
        if s.camera.exposure_warning {
            diag_row = diag_row.child(diagnostic_hud_pill(
                k,
                "hud-clip",
                "CLIPPING",
                k.c.danger.fg,
            ));
        }
        if s.camera.focus_mask {
            diag_row = diag_row.child(diagnostic_hud_pill(
                k,
                "hud-focus",
                "FOCUS MASK",
                k.c.success.fg,
            ));
        }
        if s.camera.crop_mode {
            diag_row = diag_row.child(diagnostic_hud_pill(k, "hud-crop", "CROP", k.c.text_accent));
        }
        container = container.child(diag_row);
    }

    if let Some(param) = s.speed_edit {
        let pill = k.elevate(
            row()
                .key("hud-speed-edit")
                .items_center()
                .gap(space::LG)
                .px(space::XXL)
                .py(space::CONTROL)
                .radius(k.radius.lg)
                .continuous_corners()
                .backdrop_blur(size::HUD_BLUR_STRONG)
                .fill(color::hud_surface_strong(k))
                .border(border::HAIRLINE, k.c.border_focus)
                .child(
                    Keybind::new(&[param.key_label()])
                        .variant(KeybindVariant::Neutral)
                        .build(k),
                )
                .child(
                    text("SPEED EDIT")
                        .font(ty::section_header(k))
                        .color(k.c.text_muted),
                )
                .child(text("•").font(ty::caption(k)).color(k.c.text_faint))
                .child(
                    text(param.label())
                        .font(ty::control_label_medium(k))
                        .color(k.c.text_base),
                )
                .child(
                    text(param.format_value(&s.recipe))
                        .font(ty::mono_hud(k))
                        .color(k.c.text_accent),
                )
                .child(
                    text("Drag or scroll")
                        .font(ty::caption(k))
                        .color(k.c.text_faint),
                ),
            &k.elevation.floating,
        );
        container = container.child(
            row()
                .pin(Some(0.0), None, Some(0.0), Some(space::HUD_X))
                .justify_center()
                .child(pill),
        );
    } else if s.wb_eyedropper_active {
        let pill = k.elevate(
            row()
                .key("hud-wb-picker")
                .items_center()
                .gap(space::MD)
                .px(space::XXL)
                .py(space::CONTROL)
                .radius(k.radius.lg)
                .continuous_corners()
                .backdrop_blur(size::HUD_BLUR_STRONG)
                .fill(color::hud_surface_strong(k))
                .border(border::HAIRLINE, k.c.border_focus)
                .child(
                    text("WB EYEDROPPER")
                        .font(ty::section_header(k))
                        .color(k.c.text_accent),
                )
                .child(
                    text("Click neutral gray on photo  •  Esc to cancel")
                        .font(ty::control_label(k))
                        .color(k.c.text_muted),
                ),
            &k.elevation.floating,
        );
        container = container.child(
            row()
                .pin(Some(0.0), None, Some(0.0), Some(space::HUD_X))
                .justify_center()
                .child(pill),
        );
    }

    container
}

fn status_divider(k: &Kit) -> El {
    row()
        .h(size::DIVIDER_STATUS_H)
        .items_center()
        .child(Divider::vertical().build(k))
}

fn status_key_hint(k: &Kit, keys: &[&str], label: &'static str) -> El {
    row()
        .items_center()
        .gap(space::XS)
        .child(Keybind::new(keys).variant(KeybindVariant::Neutral).build(k))
        .child(text(label).font(ty::caption(k)).color(k.c.text_faint))
}

fn status_bar(s: &AppState, k: &Kit) -> El {
    let mono = ty::mono_status(k);

    let zoom_label = if (s.camera.zoom - 1.0).abs() < 0.02 {
        "Zoom Fit".to_owned()
    } else {
        let zoom_pct = (s.camera.zoom * 100.0).round() as i32;
        format!("Zoom {zoom_pct}%")
    };
    let rot_str = if s.camera.rotation_deg.abs() > 0.1 {
        format!("  ·  {:.1}°", s.camera.rotation_deg)
    } else {
        String::new()
    };

    let mut left = row()
        .items_center()
        .gap(space::MD)
        .min_w(0.0)
        .shrink()
        .child(
            text(format!(
                "{}×{}  ·  {}",
                s.image.width, s.image.height, s.image.format_label
            ))
            .font(mono)
            .color(k.c.text_muted),
        );

    if let Some(exif) = s.image.exif.as_ref() {
        let mut exif_parts = Vec::with_capacity(3);
        if let Some(cam) = exif.camera_model.as_deref() {
            exif_parts.push(cam.to_owned());
        }
        if let Some(lens) = exif.lens_model.as_deref() {
            exif_parts.push(lens.to_owned());
        }
        let exp = exif.exposure_summary();
        if !exp.is_empty() {
            exif_parts.push(exp);
        }
        if !exif_parts.is_empty() {
            left = left.child(status_divider(k)).child(
                text(exif_parts.join("  ·  "))
                    .font(mono)
                    .color(k.c.text_base)
                    .truncate(),
            );
        }
    }

    if let Some(msg) = s.status_message.as_deref()
        && !msg.is_empty()
    {
        left = left.child(status_divider(k)).child(
            text(msg)
                .font(ty::caption(k))
                .color(k.c.text_muted)
                .truncate(),
        );
    }

    let right = row()
        .items_center()
        .gap(space::LG)
        .child(
            text(format!("{zoom_label}{rot_str}"))
                .font(mono)
                .color(k.c.text_muted),
        )
        .child(status_divider(k))
        .child(status_key_hint(k, &["Q"], "Speed Edit"))
        .child(status_key_hint(k, &["C"], "Crop"))
        .child(status_key_hint(k, &["\\"], "Compare"))
        .child(status_key_hint(k, &["⌘", "Z"], "Undo"));

    row()
        .key("status-bar")
        .role("status")
        .w_full()
        .h(size::STATUS_BAR_H)
        .px(space::XXL)
        .items_center()
        .justify_between()
        .gap(space::XL)
        .fill(k.c.bg_deep)
        .border_sides(
            [border::REGULAR, border::NONE, border::NONE, border::NONE],
            k.c.border_base,
        )
        .child(left)
        .child(right)
}

fn about_dialog_overlay(k: &Kit) -> El {
    let card = k.elevate(
        col()
            .key("about-card")
            .role("dialog")
            .label("About Rune Darkroom")
            .w(size::DIALOG_ABOUT_W)
            .gap(space::XL)
            .padding(space::MODAL_PAD)
            .radius(k.radius.xl)
            .continuous_corners()
            .fill(k.c.bg_base)
            .border(border::HAIRLINE, k.c.border_strong)
            .child(
                text("Rune Darkroom")
                    .font(ty::dialog_title(k))
                    .color(k.c.text_base),
            )
            .child(
                text("GPU-accelerated 16-bit linear FP16 photo darkroom powered by Rune, wgpu, and Anomaly OpenCode design tokens.")
                    .font(ty::dialog_body(k))
                    .color(k.c.text_muted)
                    .wrap(),
            )
            .child(
                row().w_full().justify_end().child(
                    Button::new("Done")
                        .variant(ButtonVariant::Contrast)
                        .control_size(ControlSize::Regular)
                        .on_press(|s: &mut AppState| s.show_about = false)
                        .build(k),
                ),
            ),
        &k.elevation.overlay,
    );

    scrim_tx("about-scrim", |s: &mut AppState, _| {
        s.show_about = false;
    })
    .fill(k.c.scrim)
    .center()
    .child(card)
}

fn close_confirm_overlay(k: &Kit) -> El {
    let card = k.elevate(
        col()
            .key("close-confirm-card")
            .role("dialog")
            .label("Unsaved adjustments")
            .w(size::DIALOG_CLOSE_W)
            .gap(space::XL)
            .padding(space::MODAL_PAD)
            .radius(k.radius.xl)
            .continuous_corners()
            .fill(k.c.bg_base)
            .border(border::HAIRLINE, k.c.border_strong)
            .child(
                text("Save adjustments before closing?")
                    .font(ty::dialog_title(k))
                    .color(k.c.text_base),
            )
            .child(
                text("Your develop adjustments have not been saved to a .rune-edit.json sidecar yet.")
                    .font(ty::dialog_body(k))
                    .color(k.c.text_muted)
                    .wrap(),
            )
            .child(
                row()
                    .w_full()
                    .justify_end()
                    .gap(space::MD)
                    .child(
                        Button::new("Cancel")
                            .variant(ButtonVariant::Ghost)
                            .control_size(ControlSize::Regular)
                            .on_press(|s: &mut AppState| s.confirm_close = false)
                            .build(k),
                    )
                    .child(
                        Button::new("Discard")
                            .variant(ButtonVariant::Danger)
                            .control_size(ControlSize::Regular)
                            .on_press_tx(|s: &mut AppState, tx| {
                                s.confirm_close = false;
                                s.saved_recipe = s.recipe.clone();
                                tx.close_window();
                            })
                            .build(k),
                    )
                    .child(
                        Button::new("Save Sidecar")
                            .variant(ButtonVariant::Contrast)
                            .control_size(ControlSize::Regular)
                            .on_press_tx(|s: &mut AppState, tx| {
                                s.save_sidecar_tx(tx);
                                s.confirm_close = false;
                                tx.close_window();
                            })
                            .build(k),
                    ),
            ),
        &k.elevation.overlay,
    );

    scrim_tx("close-scrim", |s: &mut AppState, _| {
        s.confirm_close = false;
    })
    .fill(k.c.scrim)
    .center()
    .child(card)
}

fn handle_key(s: &mut AppState, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let cmd = ev.mods.cmd || ev.mods.ctrl;
    if (cmd || ev.mods.alt) && s.speed_edit.is_some() {
        s.end_speed_edit_tx(tx);
    }

    match &ev.key {
        Key::Escape => {
            if s.show_about {
                s.show_about = false;
                return true;
            }
            if s.confirm_close {
                s.confirm_close = false;
                return true;
            }
            if s.speed_edit.is_some() {
                s.end_speed_edit_tx(tx);
                return true;
            }
            if s.wb_eyedropper_active {
                s.wb_eyedropper_active = false;
                s.status_message = Some("Cancelled WB Eyedropper".to_owned());
                return true;
            }
            if s.camera.crop_mode {
                s.toggle_crop_mode_tx(tx);
                return true;
            }
            false
        }
        _ if s.show_about || s.confirm_close => false,
        Key::Left if !cmd => {
            s.select_prev_photo_tx(tx);
            true
        }
        Key::Right if !cmd => {
            s.select_next_photo_tx(tx);
            true
        }
        Key::Text(ch) if ch == "\\" && !cmd => {
            s.camera.split_compare = !s.camera.split_compare;
            s.sync_window_state(tx);
            true
        }
        Key::Text(ch) if !cmd && !ev.mods.alt && ch.eq_ignore_ascii_case("f") => {
            s.show_filmstrip = !s.show_filmstrip;
            s.sync_window_state(tx);
            true
        }
        Key::Text(ch) if !cmd && !ev.mods.alt && ch.eq_ignore_ascii_case("i") => {
            s.show_inspector = !s.show_inspector;
            s.sync_window_state(tx);
            true
        }
        Key::Text(ch) if !cmd && !ev.mods.alt && ch.eq_ignore_ascii_case("c") => {
            s.toggle_crop_mode_tx(tx);
            true
        }
        Key::Text(ch) if !cmd && !ev.mods.alt && ch.eq_ignore_ascii_case("p") => {
            s.toggle_active_flag_tx(CullFlag::Pick, tx);
            true
        }
        Key::Text(ch) if !cmd && !ev.mods.alt && ch.eq_ignore_ascii_case("x") => {
            s.toggle_active_flag_tx(CullFlag::Reject, tx);
            true
        }
        Key::Text(ch)
            if !cmd && !ev.mods.alt && matches!(ch.as_str(), "0" | "1" | "2" | "3" | "4" | "5") =>
        {
            if let Ok(r) = ch.parse::<u8>() {
                s.set_active_rating_tx(r, tx);
                return true;
            }
            false
        }
        Key::Text(ch) if !cmd && !ev.mods.alt => {
            if let Some(param) = SpeedEditParam::from_key(ch) {
                s.begin_speed_edit(param);
                return true;
            }
            false
        }
        Key::Text(ch) if cmd && ch.eq_ignore_ascii_case("z") => {
            if ev.mods.shift {
                s.redo_tx(tx);
            } else {
                s.undo_tx(tx);
            }
            true
        }
        Key::Text(ch) if cmd && ev.mods.shift && ch.eq_ignore_ascii_case("o") => {
            trigger_open_folder(tx);
            true
        }
        Key::Text(ch) if cmd && ch.eq_ignore_ascii_case("o") => {
            trigger_open_dialog(tx);
            true
        }
        Key::Text(ch) if cmd && ev.mods.shift && ch.eq_ignore_ascii_case("e") => {
            trigger_export_dialog(s, tx);
            true
        }
        Key::Text(ch) if cmd && !ev.mods.shift && ch.eq_ignore_ascii_case("e") => {
            s.toggle_exposure_warning_tx(tx);
            true
        }
        Key::Text(ch) if cmd && !ev.mods.shift && ch.eq_ignore_ascii_case("m") => {
            s.toggle_focus_mask_tx(tx);
            true
        }
        Key::Text(ch) if cmd && ch.eq_ignore_ascii_case("s") => {
            s.save_sidecar_tx(tx);
            true
        }
        Key::Text(ch) if cmd && ev.mods.shift && ch.eq_ignore_ascii_case("c") => {
            s.copy_adjustments_tx(tx);
            true
        }
        Key::Text(ch) if cmd && ev.mods.shift && ch.eq_ignore_ascii_case("v") => {
            s.paste_adjustments_tx(tx);
            true
        }
        Key::Text(ch) if cmd && ch.eq_ignore_ascii_case("r") => {
            s.reset_recipe_tx(tx);
            true
        }
        Key::Text(ch) if cmd && ch == "0" => {
            s.camera.reset();
            true
        }
        Key::Text(ch) if cmd && ch == "1" => {
            s.camera.zoom = 1.5;
            true
        }
        Key::Text(ch) if cmd && ch == "2" => {
            s.camera.zoom = 2.0;
            true
        }
        _ => false,
    }
}

fn handle_key_up(s: &mut AppState, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if let Some(active) = s.speed_edit {
        let modifier_exit = ev.mods.alt || ev.mods.ctrl || ev.mods.cmd;
        let matching_key =
            matches!(&ev.key, Key::Text(ch) if SpeedEditParam::from_key(ch) == Some(active));
        if modifier_exit || matching_key {
            s.end_speed_edit_tx(tx);
            return matching_key;
        }
    }
    false
}

fn view(s: &AppState, cx: &mut Cx<AppState>) -> El {
    let k = tokens::kit();
    let insets = cx.chrome_insets();

    let mut stage_col = col()
        .key("stage-column")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .child(photo_viewport(s, k));

    if s.show_filmstrip {
        stage_col = stage_col.child(filmstrip_bar(s, k));
    }

    let mut body = row()
        .key("workspace-body")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(stage_col);

    if s.show_inspector {
        body = body.child(right_inspector(s, k));
    }

    let base = col()
        .key("root")
        .w_full()
        .h_full()
        .fill(k.c.page)
        .on_key(handle_key)
        .on_key_up(handle_key_up)
        .child(top_toolbar(s, k, insets))
        .child(body)
        .child(status_bar(s, k));

    let mut overlays = Vec::new();
    if s.show_about {
        overlays.push(about_dialog_overlay(k));
    }
    if s.confirm_close {
        overlays.push(close_confirm_overlay(k));
    }

    rune_kit::root(base, overlays)
}

fn scene_editorial(s: &mut AppState) {
    s.active_tab = InspectorTab::Light;
    s.wb_open = true;
    s.tone_open = true;
    s.levels_open = false;
    s.curve_open = false;
    s.recipe.temperature = 0.06;
    s.recipe.tint = -0.02;
    s.recipe.exposure = 0.12;
    s.recipe.contrast = 0.14;
    s.recipe.highlights = -0.18;
    s.recipe.shadows = 0.12;
    s.recipe.whites = 0.06;
    s.recipe.blacks = -0.05;
    s.recipe.vibrance = 0.10;
    s.recipe.vignette = -0.08;
    s.recipe.preset_name = None;
    s.status_message = Some("Subtle editorial grade • 16-bit Linear FP16".to_owned());
    s.recompute_histogram();
}

fn scene_color(s: &mut AppState) {
    s.active_tab = InspectorTab::Color;
    s.bw_open = false;
    s.selected_hsl_band = 1;
    s.recipe.exposure = 0.08;
    s.recipe.contrast = 0.12;
    s.recipe.highlights = -0.14;
    s.recipe.shadows = 0.10;
    s.recipe.vibrance = 0.12;
    s.recipe.saturation = -0.04;
    s.recipe.shadows_wheel = ColorWheel::new(-0.18, -0.12, -0.04);
    s.recipe.midtones_wheel = ColorWheel::new(0.14, 0.08, 0.02);
    s.recipe.highlights_wheel = ColorWheel::new(0.24, 0.14, 0.04);
    s.recipe.hsl[1] = HslBand::new(-0.06, 0.16, 0.08);
    s.recipe.preset_name = None;
    s.status_message = Some("3-Way Color Balance + Skin/Ceramic HSL".to_owned());
    s.recompute_histogram();
}

fn scene_tone_curve(s: &mut AppState) {
    s.active_tab = InspectorTab::Light;
    s.wb_open = false;
    s.tone_open = false;
    s.levels_open = false;
    s.curve_open = true;
    s.curve_channel = CurveChannel::Luma;
    s.recipe.exposure = 0.20;
    s.recipe.contrast = 0.15;
    s.recipe.highlights = -0.35;
    s.recipe.shadows = 0.25;
    s.recipe.curves = ToneCurves {
        luma: vec![
            [0.0, 0.04],
            [0.25, 0.18],
            [0.52, 0.54],
            [0.78, 0.84],
            [1.0, 0.97],
        ],
        blue: vec![[0.0, 0.05], [0.50, 0.48], [1.0, 0.94]],
        ..ToneCurves::default()
    };
    s.recompute_histogram();
}

fn scene_split(s: &mut AppState) {
    s.apply_preset("moody_dusk");
    s.camera.split_compare = true;
    s.camera.split_pos = 0.5;
    s.active_tab = InspectorTab::Light;
}

fn scene_filmstrip(s: &mut AppState) {
    scene_editorial(s);
    s.show_filmstrip = true;
    s.active_tab = InspectorTab::Presets;
    s.recipe.preset_name = Some("Warm Film".to_owned());
    s.status_message = Some("Session culling filmstrip • 5 frames".to_owned());
}

fn scene_crop(s: &mut AppState) {
    scene_editorial(s);
    s.active_tab = InspectorTab::Crop;
    s.camera.crop_mode = true;
    s.recipe.geometry.crop = [0.08, 0.08, 0.84, 0.84];
    s.recipe
        .geometry
        .apply_aspect_preset(CropAspect::FourThree, s.image.width, s.image.height);
    s.recipe.geometry.straighten_deg = 2.4;
    s.recipe.geometry.grid = CompositionGrid::RuleOfThirds;
    s.status_message = Some("Interactive Crop & Straighten • Rule of Thirds".to_owned());
}

fn scene_levels(s: &mut AppState) {
    s.active_tab = InspectorTab::Light;
    s.wb_open = false;
    s.tone_open = true;
    s.levels_open = true;
    s.curve_open = false;
    s.recipe.brightness = 0.16;
    s.recipe.contrast = 0.12;
    s.recipe.levels = auto_levels(&s.image.histogram_proxy);
    s.recipe.preset_name = None;
    s.status_message = Some("Levels & Midtone Brightness • Auto 0.1% clip".to_owned());
    s.recompute_histogram();
}

fn scene_bw(s: &mut AppState) {
    s.active_tab = InspectorTab::Color;
    s.bw_open = true;
    s.recipe.contrast = 0.24;
    s.recipe.clarity = 0.18;
    s.recipe.structure = 0.22;
    s.recipe.bw.enabled = true;
    s.recipe.bw.red = 0.34;
    s.recipe.bw.yellow = 0.20;
    s.recipe.bw.cyan = -0.25;
    s.recipe.bw.blue = -0.45;
    s.recipe.preset_name = None;
    s.status_message = Some("6-Channel Black & White Color Sensitivity".to_owned());
    s.recompute_histogram();
}

fn scene_warnings(s: &mut AppState) {
    s.active_tab = InspectorTab::Detail;
    s.recipe.exposure = 2.40;
    s.recipe.contrast = 0.35;
    s.recipe.blacks = -0.65;
    s.recipe.structure = 0.28;
    s.recipe.dehaze = 0.18;
    s.camera.exposure_warning = true;
    s.camera.focus_mask = true;
    s.speed_edit = Some(SpeedEditParam::Exposure);
    s.cursor_readout = Some(CursorColorReadout {
        rgb8: [218, 184, 152],
        hsl: [29.0, 44.0, 73.0],
        lab: [76.0, 8.0, 21.0],
    });
    s.status_message = Some("Viewer diagnostics: Clipping + Focus Mask + Speed Edit".to_owned());
    s.recompute_histogram();
}

fn main() {
    let spec = AppSpec::<AppState> {
        min_size: Some(size::WINDOW_MIN),
        fonts: rune_theme_opencode::FONTS,
        families: rune_theme_opencode::FAMILIES,
        scenes: &[
            ("editorial", scene_editorial),
            ("color", scene_color),
            ("tone-curve", scene_tone_curve),
            ("split", scene_split),
            ("filmstrip", scene_filmstrip),
            ("crop", scene_crop),
            ("levels", scene_levels),
            ("bw", scene_bw),
            ("warnings", scene_warnings),
            ("warm-film", |s| s.apply_preset("warm_film")),
            ("editorial-bw", |s| s.apply_preset("editorial_bw")),
        ],
        ..AppSpec::new(
            "Rune Darkroom",
            size::WINDOW_DEFAULT,
            AppState::default,
            view,
        )
    };

    let mut initial_state = AppState::default();
    let initial_menus = build_menu_items(&initial_state);

    let mut app = App::new(spec)
        .color_scheme(ColorScheme::Dark)
        .titlebar(Titlebar::new(size::TITLEBAR_H))
        .standard_menus()
        .macos_window(MacWindowOptions::default().color_space(MacColorSpace::Srgb))
        .prewarm_effects(PrewarmEffects::BACKDROP_BLUR)
        .compositor(DarkroomCompositor::default())
        .on_focus(|s, focused, tx| {
            if !focused {
                s.end_speed_edit_tx(tx);
            }
        })
        .on_close_request(|s, _| {
            if s.is_dirty() && s.image.source_path.is_some() {
                s.confirm_close = true;
                CloseResponse::Defer
            } else {
                CloseResponse::Close
            }
        })
        .on_open_files(|s, files, tx| {
            if let Some(first) = files.first() {
                if first.is_dir() {
                    s.open_folder_tx(first.clone(), tx);
                } else {
                    s.open_image_path_tx(first.clone(), tx);
                }
            }
        });

    for item in initial_menus {
        app = app.menu_item(item);
    }

    let handle = app.handle();
    app.init(move || {
        initial_state.app_handle = Some(handle);
        initial_state
    })
    .run();
}
