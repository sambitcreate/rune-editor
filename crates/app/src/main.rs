//! Rune Darkroom — Anomaly OpenCode-styled, Apple macOS HIG-compliant GPU-accelerated
//! Capture One / Lightroom darkroom built on Rune.
//!
//! Run interactively:
//!   cargo run -p rune-editor --release
//!
//! Render headless verification snapshots:
//!   cargo run -p rune-editor -- --chrome --snap darkroom.png
//!   cargo run -p rune-editor -- --chrome --snap color.png --scene color
//!   cargo run -p rune-editor -- --chrome --snap curve.png --scene tone-curve
//!   cargo run -p rune-editor -- --chrome --snap split.png --scene split
//!   cargo run -p rune-editor -- --chrome --snap filmstrip.png --scene filmstrip

mod color_wheel_tex;
mod compositor;
mod filmstrip;
mod menu;
mod state;
mod toolbar;
mod widgets;

use rune::{
    App, AppSpec, CloseResponse, Color, ColorScheme, Cursor, Cx, Drag, El, Key, KeyEvent,
    MacColorSpace, MacWindowOptions, MagnifyEvent, Phase, PrewarmEffects, RotateEvent, TextStyle,
    Titlebar, Tx, WheelEvent, col, effect, el, row, text,
};
use rune_editor_core::{ColorWheel, CullFlag, HslBand, ToneCurves};
use rune_kit::{
    Button, ButtonVariant, ControlSize, Divider, Keybind, KeybindVariant, Kit, scrim_tx,
};

use crate::compositor::DarkroomCompositor;
use crate::filmstrip::filmstrip_bar;
use crate::menu::{
    build_menu_items, trigger_export_dialog, trigger_open_dialog, trigger_open_folder,
};
use crate::state::{AppState, CurveChannel, InspectorTab, PHOTO_VIEWPORT_MARK};
use crate::toolbar::{TITLEBAR_HEIGHT, top_toolbar};
use crate::widgets::right_inspector;

/// Central color-accurate GPU photo viewport surrounded by opaque neutral `#121212` matte
/// (`k.c.page`), strictly obeying Apple HIG's rule against vibrancy/translucency behind
/// color-critical canvases.
fn photo_viewport(s: &AppState, k: &Kit) -> El {
    let gpu_slot = effect(PHOTO_VIEWPORT_MARK)
        .key("photo-gpu-slot")
        .w_full()
        .h_full()
        .cursor(if s.camera.zoom > 1.01 {
            Cursor::Grab
        } else {
            Cursor::Default
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
        .on_wheel(|s: &mut AppState, ev: WheelEvent, _| {
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
        .on_drag(|s: &mut AppState, d: &mut Drag, _| match d.phase {
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
                } else if let Some((lx, ly)) = s.last_drag_pos {
                    let dx = (d.x - lx) / d.w.max(1.0) * 2.0;
                    let dy = (d.y - ly) / d.h.max(1.0) * 2.0;
                    s.camera.pan_by(dx, dy);
                }
                s.last_drag_pos = Some((d.x, d.y));
            }
            Phase::Up => {
                s.last_drag_pos = None;
            }
        });

    let mut container = el()
        .key("viewport-container")
        .grow(1.0)
        .w_full()
        .min_h(0.0)
        .fill(k.c.page)
        .border(
            if s.drop_hover { 2.0 } else { 0.0 },
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

    // Floating translucent OpenCode HUD pills when Before/After Split Compare is active
    if s.camera.split_compare {
        let before_pill = k.elevate(
            el().key("hud-before")
                .pin(Some(18.0), Some(16.0), None, None)
                .px(10.0)
                .py(4.0)
                .radius(k.radius.md)
                .continuous_corners()
                .backdrop_blur(12.0)
                .fill(k.c.bg_base.alpha(0.78))
                .border(0.5, k.c.border_base)
                .child(text("BEFORE").font(k.ty.small_medium).color(k.c.text_muted)),
            &k.elevation.floating,
        );
        let after_pill = k.elevate(
            el().key("hud-after")
                .pin(None, Some(16.0), Some(18.0), None)
                .px(10.0)
                .py(4.0)
                .radius(k.radius.md)
                .continuous_corners()
                .backdrop_blur(12.0)
                .fill(k.c.bg_base.alpha(0.78))
                .border(0.5, k.c.border_focus)
                .child(text("AFTER").font(k.ty.small_medium).color(k.c.text_accent)),
            &k.elevation.floating,
        );
        container = container.child(before_pill).child(after_pill);
    }

    container
}

fn status_bar(s: &AppState, k: &Kit) -> El {
    let mono = TextStyle {
        size: 11.0,
        line_height: 14.0,
        ..k.ty.mono
    }
    .features("'tnum' 1");

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

    let mut left = row().items_center().gap(8.0).min_w(0.0).shrink().child(
        text(format!(
            "{}×{}  ·  {}",
            s.image.width, s.image.height, s.image.format_label
        ))
        .font(mono)
        .color(k.c.text_muted),
    );

    if let Some(msg) = s.status_message.as_deref()
        && !msg.is_empty()
    {
        left = left
            .child(
                row()
                    .h(12.0)
                    .items_center()
                    .child(Divider::vertical().build(k)),
            )
            .child(text(msg).font(k.ty.small).color(k.c.text_muted).truncate());
    }

    let right = row()
        .items_center()
        .gap(10.0)
        .child(
            text(format!("{zoom_label}{rot_str}"))
                .font(mono)
                .color(k.c.text_muted),
        )
        .child(
            row()
                .h(12.0)
                .items_center()
                .child(Divider::vertical().build(k)),
        )
        .child(
            row()
                .items_center()
                .gap(4.0)
                .child(
                    Keybind::new(&["\\"])
                        .variant(KeybindVariant::Neutral)
                        .build(k),
                )
                .child(text("Compare").font(k.ty.small).color(k.c.text_faint)),
        )
        .child(
            row()
                .items_center()
                .gap(4.0)
                .child(
                    Keybind::new(&["F"])
                        .variant(KeybindVariant::Neutral)
                        .build(k),
                )
                .child(text("Filmstrip").font(k.ty.small).color(k.c.text_faint)),
        )
        .child(
            row()
                .items_center()
                .gap(4.0)
                .child(
                    Keybind::new(&["⌘", "Z"])
                        .variant(KeybindVariant::Neutral)
                        .build(k),
                )
                .child(text("Undo").font(k.ty.small).color(k.c.text_faint)),
        );

    row()
        .key("status-bar")
        .role("status")
        .w_full()
        .h(28.0)
        .px(14.0)
        .items_center()
        .justify_between()
        .gap(12.0)
        .fill(k.c.bg_deep)
        .border_sides([1.0, 0.0, 0.0, 0.0], k.c.border_base)
        .child(left)
        .child(right)
}

fn about_dialog_overlay(k: &Kit) -> El {
    let card = k.elevate(
        col()
            .key("about-card")
            .role("dialog")
            .label("About Rune Darkroom")
            .w(360.0)
            .gap(12.0)
            .padding(20.0)
            .radius(k.radius.xl)
            .continuous_corners()
            .fill(k.c.bg_base)
            .border(0.5, k.c.border_strong)
            .child(
                text("Rune Darkroom")
                    .font(k.ty.body_medium)
                    .color(k.c.text_base),
            )
            .child(
                text("GPU-accelerated 16-bit linear FP16 photo darkroom powered by Rune, wgpu, and Anomaly OpenCode design tokens.")
                    .font(k.ty.text_small)
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
            .w(380.0)
            .gap(12.0)
            .padding(20.0)
            .radius(k.radius.xl)
            .continuous_corners()
            .fill(k.c.bg_base)
            .border(0.5, k.c.border_strong)
            .child(
                text("Save adjustments before closing?")
                    .font(k.ty.body_medium)
                    .color(k.c.text_base),
            )
            .child(
                text("Your develop adjustments have not been saved to a .rune-edit.json sidecar yet.")
                    .font(k.ty.text_small)
                    .color(k.c.text_muted)
                    .wrap(),
            )
            .child(
                row()
                    .w_full()
                    .justify_end()
                    .gap(8.0)
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
            false
        }
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

fn view(s: &AppState, cx: &mut Cx<AppState>) -> El {
    let k = rune_theme_opencode::kit(true);
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
    // Subtle, tasteful editorial grade for the README hero screenshot
    s.active_tab = InspectorTab::Light;
    s.wb_open = true;
    s.tone_open = true;
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
    s.selected_hsl_band = 1; // Orange band (natural skin tones & ceramic cup)
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
    s.tone_open = true;
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
    s.bump_curve_generation();
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

fn main() {
    let spec = AppSpec::<AppState> {
        min_size: Some((1024.0, 680.0)),
        fonts: rune_theme_opencode::FONTS,
        families: rune_theme_opencode::FAMILIES,
        scenes: &[
            ("editorial", scene_editorial),
            ("color", scene_color),
            ("tone-curve", scene_tone_curve),
            ("split", scene_split),
            ("filmstrip", scene_filmstrip),
            ("warm-film", |s| s.apply_preset("warm_film")),
            ("editorial-bw", |s| s.apply_preset("editorial_bw")),
        ],
        ..AppSpec::new("Rune Darkroom", (1360.0, 860.0), AppState::default, view)
    };

    let initial_state = AppState::default();
    let initial_menus = build_menu_items(&initial_state);

    let mut app = App::new(spec)
        .color_scheme(ColorScheme::Dark)
        .titlebar(Titlebar::new(TITLEBAR_HEIGHT))
        .standard_menus()
        .macos_window(MacWindowOptions::default().color_space(MacColorSpace::Srgb))
        .prewarm_effects(PrewarmEffects::BACKDROP_BLUR)
        .compositor(DarkroomCompositor::default())
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
    app.init(move || AppState::with_handle(handle)).run();
}
