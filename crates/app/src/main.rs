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

mod browser;
mod color_wheel_tex;
mod compositor;
mod export_view;
mod filmstrip;
mod layers_panel;
mod mcp_bridge;
mod menu;
mod settings_view;
mod state;
mod tokens;
mod toolbar;
mod widgets;

use rune::{
    App, AppSpec, CloseResponse, Color, ColorScheme, Cursor, Cx, Drag, El, Key, KeyEvent, LineCap,
    MacColorSpace, MacWindowOptions, MagnifyEvent, Phase, PointerEvent, PrewarmEffects,
    RotateEvent, Shape, SurfaceFit, Titlebar, Tx, WheelEvent, col, effect, el, row, text,
};
use rune_editor_core::{
    AdjustmentLayer, ClarityMethod, ColorTag, ColorWheel, CompositionGrid, CropAspect, CullFlag,
    FilmCurve, FilmNegativeParams, GrainType, HealMode, HealSpot, HslBand, LayerMaskItem,
    LocalAdjustments, MaskCombineOp, MaskShape, ProcessingMode, ToneCurves, UserStyle,
    VignetteMethod, WbPreset, auto_levels,
};
use rune_editor_decode::{ExportColorSpace, WatermarkPosition};
use rune_kit::{
    Button, ButtonVariant, ControlSize, Divider, Keybind, KeybindVariant, Kit, scrim_tx,
};

use crate::browser::grid_browser_view;
use crate::compositor::DarkroomCompositor;
use crate::export_view::export_view;
use crate::filmstrip::filmstrip_bar;
use crate::menu::{
    build_menu_items, trigger_export_dialog, trigger_open_dialog, trigger_open_folder,
};
use crate::settings_view::settings_view;
use crate::state::{
    AppState, CanvasTool, ColorBalanceView, ColorEditorMode, CompareReference, CursorColorReadout,
    CurveChannel, InspectorTab, LevelsChannel, PHOTO_VIEWPORT_MARK, PinnedReadout, ProofMargin,
    SettingsSection, ShortcutAction, SpeedEditParam,
};
use crate::tokens::{border, color, radius, size, space, ty};
use crate::toolbar::{overlays_popover_overlay, top_toolbar};
use crate::widgets::{floating_info_popover, right_inspector};

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
    let cursor = if s.wb_eyedropper_active
        || s.film_base_picker_active
        || s.color_slice_picker_active
        || s.skin_picker_active
        || s.readout_picker_active
        || s.camera.crop_mode
        || s.active_tool.is_retouch_or_mask()
    {
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
            if s.camera.loupe_enabled {
                s.update_loupe_uv(p.x, p.y);
            }
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
            if s.film_base_picker_active {
                if matches!(d.phase, Phase::Down) {
                    s.pick_film_base_at_viewport_tx(d.x, d.y, tx);
                }
                return;
            }
            if s.color_slice_picker_active {
                if matches!(d.phase, Phase::Down) {
                    s.pick_color_slice_at_viewport_tx(d.x, d.y, tx);
                }
                return;
            }
            if s.skin_picker_active {
                if matches!(d.phase, Phase::Down) {
                    s.pick_skin_tone_at_viewport_tx(d.x, d.y, tx);
                }
                return;
            }
            if s.readout_picker_active {
                if matches!(d.phase, Phase::Down) {
                    s.pin_color_readout_at_viewport_tx(d.x, d.y, tx);
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

            if s.active_tool.is_retouch_or_mask() {
                match d.phase {
                    Phase::Down => {
                        s.begin_canvas_tool_drag(d.x, d.y);
                        tx.instant();
                    }
                    Phase::Move => {
                        s.update_canvas_tool_drag(d.x, d.y);
                        tx.instant();
                    }
                    Phase::Up => s.end_canvas_tool_drag_tx(tx),
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

    let mat_pad = s.proof_margin.extra_padding_pts();
    let mat_fill = match s.proof_margin {
        ProofMargin::Compact => k.c.page,
        ProofMargin::Studio => color::proof_margin_studio(),
        ProofMargin::Exhibition => color::proof_margin_exhibition(),
    };
    let stage_child = if mat_pad > 0.0 {
        el().key("proof-margin-frame")
            .w_full()
            .h_full()
            .fill(k.c.page)
            .border(border::HAIRLINE, k.c.border_strong)
            .child(gpu_slot)
    } else {
        gpu_slot
    };

    let mut container = el()
        .key("viewport-container")
        .grow(1.0)
        .w_full()
        .min_h(0.0)
        .padding(mat_pad)
        .fill(mat_fill)
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
        .child(stage_child);

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

    if s.camera.exposure_warning
        || s.camera.focus_mask
        || s.camera.crop_mode
        || s.camera.view_color_range >= 0
        || s.camera.loupe_enabled
        || s.camera.mask_overlay_mode != 0
        || s.active_tool.is_retouch_or_mask()
        || s.recipe.processing_mode == ProcessingMode::FilmNegative
        || s.proof_margin != ProofMargin::Compact
        || s.compare_reference.is_some()
    {
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
        if s.compare_reference.is_some() {
            diag_row = diag_row.child(diagnostic_hud_pill(
                k,
                "hud-candidate",
                "CANDIDATE",
                k.c.text_base,
            ));
        }
        if s.recipe.processing_mode == ProcessingMode::FilmNegative {
            diag_row = diag_row.child(diagnostic_hud_pill(
                k,
                "hud-film-negative",
                "FILM NEGATIVE",
                color::film_negative_accent(),
            ));
        }
        if s.proof_margin == ProofMargin::Studio {
            diag_row = diag_row.child(diagnostic_hud_pill(
                k,
                "hud-proof-margin",
                "PROOF MARGIN · STUDIO",
                k.c.text_base,
            ));
        } else if s.proof_margin == ProofMargin::Exhibition {
            diag_row = diag_row.child(diagnostic_hud_pill(
                k,
                "hud-proof-margin",
                "PROOF MARGIN · EXHIBITION",
                k.c.text_base,
            ));
        }
        if s.camera.mask_overlay_mode == 1 {
            diag_row = diag_row.child(diagnostic_hud_pill(
                k,
                "hud-mask-red",
                "MASK: RED (M)",
                color::mask_overlay_red(),
            ));
        } else if s.camera.mask_overlay_mode == 2 {
            diag_row = diag_row.child(diagnostic_hud_pill(
                k,
                "hud-mask-gray",
                "MASK: GRAY (⌥M)",
                color::mask_overlay_gray(),
            ));
        }
        if s.active_tool.is_retouch_or_mask() {
            diag_row = diag_row.child(diagnostic_hud_pill(
                k,
                "hud-canvas-tool",
                s.active_tool.hud_badge(),
                color::mask_guide_primary(),
            ));
        }
        if s.camera.loupe_enabled {
            diag_row = diag_row.child(diagnostic_hud_pill(
                k,
                "hud-loupe",
                "LOUPE 2.5×",
                k.c.text_accent,
            ));
        }
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
        if s.camera.view_color_range >= 0 {
            diag_row = diag_row.child(diagnostic_hud_pill(
                k,
                "hud-color-range",
                "COLOR RANGE",
                color::color_range_hud(),
            ));
        }
        container = container.child(diag_row);
    }

    // Phase 4 vector canvas guides (Linear Gradient 3-line guide, Radial dashed ellipse,
    // and Heal/Clone target -> source circles & dashed connectors).
    if let Some(guides) = canvas_vector_guides(s) {
        container = container.child(guides);
    }

    // Pinned Color Readout markers (#1..#4) overlaid directly on the photo canvas.
    if !s.pinned_readouts.is_empty() {
        let vw = if s.viewport_rect_pts[0] > 1.0 {
            s.viewport_rect_pts[0]
        } else {
            1024.0
        };
        let vh = if s.viewport_rect_pts[1] > 1.0 {
            s.viewport_rect_pts[1]
        } else {
            784.0
        };
        let sz = size::PIN_MARKER;
        for (idx, pin) in s.pinned_readouts.iter().enumerate() {
            let px = (pin.slot_uv[0] * vw - sz * 0.5)
                .clamp(space::XS, (vw - sz - space::XS).max(space::XS));
            let py = (pin.slot_uv[1] * vh - sz * 0.5)
                .clamp(space::XS, (vh - sz - space::XS).max(space::XS));
            container = container.child(
                row()
                    .key(format!("canvas-pin-marker-{idx}"))
                    .absolute(px, py)
                    .size(sz, sz)
                    .center()
                    .radius(radius::circle(sz))
                    .fill(color::hud_surface_strong(k))
                    .border(border::ACTIVE, color::pin_marker_ring())
                    .child(
                        text(format!("{}", idx + 1))
                            .font(ty::mono_micro(k))
                            .color(k.c.white),
                    )
                    .a11y_hidden(),
            );
        }
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
    } else if s.wb_eyedropper_active
        || s.film_base_picker_active
        || s.color_slice_picker_active
        || s.skin_picker_active
        || s.readout_picker_active
    {
        let (badge, hint) = if s.wb_eyedropper_active {
            (
                "WB EYEDROPPER",
                "Click neutral gray on photo  •  Esc to cancel".to_owned(),
            )
        } else if s.film_base_picker_active {
            (
                "FILM BASE PICKER",
                "Click unexposed orange film border on photo  •  Esc to cancel".to_owned(),
            )
        } else if s.color_slice_picker_active {
            (
                "COLOR PICKER",
                format!(
                    "Click a color on photo for Slice {}  •  Esc to cancel",
                    s.selected_color_slice + 1
                ),
            )
        } else if s.skin_picker_active {
            (
                "SKIN TONE PICKER",
                "Click skin on photo to sample target  •  Esc to cancel".to_owned(),
            )
        } else {
            (
                "PIN READOUT",
                "Click photo to pin live RGB/Lab marker (#1–#4)  •  Esc to cancel".to_owned(),
            )
        };
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
                    text(badge)
                        .font(ty::section_header(k))
                        .color(k.c.text_accent),
                )
                .child(text(hint).font(ty::control_label(k)).color(k.c.text_muted)),
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

/// Renders crisp Capture One Pro vector guides directly over the GPU photo viewport:
/// - Linear Gradient: 3 parallel lines (start, center, end feather) + axis & center handles.
/// - Radial Gradient: dashed rotated ellipse + center crosshair.
/// - Spot Heal & Clone: target circle (orange Heal / cyan Clone) + dashed connector + source circle.
fn canvas_vector_guides(s: &AppState) -> Option<El> {
    let show_layer_guides = s.active_tab == InspectorTab::Layers
        || matches!(
            s.active_tool,
            CanvasTool::LinearGradient | CanvasTool::RadialGradient
        );
    let show_heal_guides = !s.recipe.heal_spots.is_empty()
        && (s.active_tab == InspectorTab::Layers
            || matches!(s.active_tool, CanvasTool::Heal | CanvasTool::Clone));

    let active_layer = if show_layer_guides {
        s.active_layer_index.and_then(|i| s.recipe.layers.get(i))
    } else {
        None
    };

    let has_gradient_guide = active_layer.is_some_and(|l| {
        l.masks.iter().any(|m| {
            m.enabled
                && matches!(
                    m.shape,
                    MaskShape::LinearGradient { .. } | MaskShape::RadialGradient { .. }
                )
        })
    });

    if !has_gradient_guide && !show_heal_guides {
        return None;
    }

    let vw = if s.viewport_rect_pts[0] > 1.0 {
        s.viewport_rect_pts[0]
    } else {
        size::WINDOW_DEFAULT.0 - size::INSPECTOR_W
    };
    let vh = if s.viewport_rect_pts[1] > 1.0 {
        s.viewport_rect_pts[1]
    } else {
        size::WINDOW_DEFAULT.1 - size::TITLEBAR_H - size::STATUS_BAR_H
    };

    let mut shape = Shape::new().view_box(vw, vh);

    if let Some(layer) = active_layer {
        for mask in &layer.masks {
            if !mask.enabled {
                continue;
            }
            match mask.shape {
                MaskShape::LinearGradient { start, end, .. } => {
                    let p0 = s.source_uv_to_viewport_local(start, vw, vh);
                    let p1 = s.source_uv_to_viewport_local(end, vw, vh);
                    let mid = [(p0[0] + p1[0]) * 0.5, (p0[1] + p1[1]) * 0.5];
                    let dx = p1[0] - p0[0];
                    let dy = p1[1] - p0[1];
                    let len = dx.hypot(dy).max(1.0);
                    let px = -dy / len;
                    let py = dx / len;
                    let half_w = (vw.min(vh) * 0.24).clamp(72.0, 220.0);

                    // 3 parallel guide lines (Start, Center, End)
                    for &(anchor, col, stroke_w) in &[
                        (p0, color::mask_guide_feather(), border::REGULAR),
                        (mid, color::mask_guide_primary(), border::ACTIVE),
                        (p1, color::mask_guide_feather(), border::REGULAR),
                    ] {
                        shape = shape.line(
                            [anchor[0] - px * half_w, anchor[1] - py * half_w],
                            [anchor[0] + px * half_w, anchor[1] + py * half_w],
                            stroke_w,
                            LineCap::Round,
                            col,
                        );
                    }
                    // Gradient direction axis + handle dots
                    shape = shape.line(
                        p0,
                        p1,
                        border::REGULAR,
                        LineCap::Round,
                        color::mask_guide_feather(),
                    );
                    for pt in [p0, mid, p1] {
                        let r = 4.0_f32;
                        let mut ring = Vec::with_capacity(13);
                        for i in 0..=12 {
                            let a = (i as f32 / 12.0) * std::f32::consts::TAU;
                            ring.push([pt[0] + a.cos() * r, pt[1] + a.sin() * r]);
                        }
                        shape = shape.stroke(ring, border::ACTIVE, color::mask_guide_primary());
                    }
                }
                MaskShape::RadialGradient {
                    center,
                    radius,
                    angle_deg,
                    ..
                } => {
                    let segs = 40usize;
                    let (sn, cs) = angle_deg.to_radians().sin_cos();
                    let sample_pt = |i: usize| {
                        let a = (i as f32 / segs as f32) * std::f32::consts::TAU;
                        let lx = a.cos() * radius[0];
                        let ly = a.sin() * radius[1];
                        let u = center[0] + lx * cs - ly * sn;
                        let v = center[1] + lx * sn + ly * cs;
                        s.source_uv_to_viewport_local([u, v], vw, vh)
                    };
                    // Dashed ellipse
                    for i in (0..segs).step_by(2) {
                        let a = sample_pt(i);
                        let b = sample_pt(i + 1);
                        shape = shape.line(
                            a,
                            b,
                            border::ACTIVE,
                            LineCap::Round,
                            color::mask_guide_primary(),
                        );
                    }
                    // Center crosshair handle
                    let c = s.source_uv_to_viewport_local(center, vw, vh);
                    let arm = 6.0_f32;
                    shape = shape
                        .line(
                            [c[0] - arm, c[1]],
                            [c[0] + arm, c[1]],
                            border::ACTIVE,
                            LineCap::Round,
                            color::mask_guide_primary(),
                        )
                        .line(
                            [c[0], c[1] - arm],
                            [c[0], c[1] + arm],
                            border::ACTIVE,
                            LineCap::Round,
                            color::mask_guide_primary(),
                        );
                }
                _ => {}
            }
        }
    }

    if show_heal_guides {
        for spot in &s.recipe.heal_spots {
            let t = s.source_uv_to_viewport_local(spot.target, vw, vh);
            let src = s.source_uv_to_viewport_local(spot.source, vw, vh);
            let edge = s.source_uv_to_viewport_local(
                [spot.target[0] + spot.radius, spot.target[1]],
                vw,
                vh,
            );
            let r_pts = (edge[0] - t[0]).hypot(edge[1] - t[1]).clamp(10.0, 120.0);
            let target_col = match spot.mode {
                HealMode::Heal => color::heal_target_ring(),
                HealMode::Clone => color::clone_target_ring(),
            };

            let circle_pts = |c: [f32; 2], r: f32| -> Vec<[f32; 2]> {
                let mut pts = Vec::with_capacity(25);
                for i in 0..=24 {
                    let a = (i as f32 / 24.0) * std::f32::consts::TAU;
                    pts.push([c[0] + a.cos() * r, c[1] + a.sin() * r]);
                }
                pts
            };

            // Target ring + Source ring
            shape = shape
                .stroke(circle_pts(t, r_pts), border::CURVE_ACTIVE, target_col)
                .stroke(
                    circle_pts(src, r_pts),
                    border::REGULAR,
                    color::heal_source_ring(),
                );

            // Dashed connector from source rim -> target rim
            let dx = t[0] - src[0];
            let dy = t[1] - src[1];
            let dist = dx.hypot(dy);
            if dist > r_pts * 2.1 {
                let ux = dx / dist;
                let uy = dy / dist;
                let p_start = [src[0] + ux * r_pts, src[1] + uy * r_pts];
                let p_end = [t[0] - ux * r_pts, t[1] - uy * r_pts];
                let dashes = 7usize;
                for i in (0..dashes).step_by(2) {
                    let t0 = i as f32 / dashes as f32;
                    let t1 = ((i + 1) as f32 / dashes as f32).min(1.0);
                    let a = [
                        p_start[0] + (p_end[0] - p_start[0]) * t0,
                        p_start[1] + (p_end[1] - p_start[1]) * t0,
                    ];
                    let b = [
                        p_start[0] + (p_end[0] - p_start[0]) * t1,
                        p_start[1] + (p_end[1] - p_start[1]) * t1,
                    ];
                    shape = shape.line(
                        a,
                        b,
                        border::REGULAR,
                        LineCap::Round,
                        color::heal_connector_line(),
                    );
                }
            }
        }
    }

    Some(
        el().key("canvas-vector-guides")
            .absolute(0.0, 0.0)
            .size(vw, vh)
            .shape(shape)
            .a11y_hidden(),
    )
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

    let mut right = row().items_center().gap(space::LG);
    if s.proof_margin != ProofMargin::Compact {
        right = right
            .child(
                text(format!("Proof: {}", s.proof_margin.label()))
                    .font(mono)
                    .color(k.c.text_base),
            )
            .child(status_divider(k));
    }
    let right = right
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

    // Interactive shortcut recording inside Settings → Shortcuts & Workflow
    if let Some(action) = s.recording_shortcut {
        match &ev.key {
            Key::Escape => {
                s.recording_shortcut = None;
                s.status_message = Some("Cancelled shortcut rebind".to_owned());
                return true;
            }
            Key::Text(ch) if !cmd && !ev.mods.alt => {
                let _ = s.set_shortcut_binding_tx(action, ch, tx);
                return true;
            }
            _ => return false,
        }
    }

    match &ev.key {
        Key::Escape => {
            if s.overlays_popover_open {
                s.overlays_popover_open = false;
                return true;
            }
            if s.show_about {
                s.show_about = false;
                return true;
            }
            if s.confirm_close {
                s.confirm_close = false;
                return true;
            }
            if s.show_settings {
                s.set_show_settings_tx(false, tx);
                return true;
            }
            if s.show_export {
                s.set_show_export_tx(false, tx);
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
            if s.film_base_picker_active {
                s.film_base_picker_active = false;
                s.status_message = Some("Cancelled Film Base Picker".to_owned());
                return true;
            }
            if s.color_slice_picker_active {
                s.color_slice_picker_active = false;
                s.status_message = Some("Cancelled Color Picker".to_owned());
                return true;
            }
            if s.skin_picker_active {
                s.skin_picker_active = false;
                s.status_message = Some("Cancelled Skin Tone Picker".to_owned());
                return true;
            }
            if s.readout_picker_active {
                s.readout_picker_active = false;
                s.status_message = Some("Pin Readout tool OFF".to_owned());
                return true;
            }
            if s.camera.view_color_range >= 0 {
                s.camera.view_color_range = -1;
                s.status_message = Some("View Selected Color Range OFF".to_owned());
                return true;
            }
            if s.camera.loupe_enabled {
                s.toggle_loupe_tx(tx);
                return true;
            }
            if s.compare_reference.is_some() {
                s.clear_compare_variant_tx(tx);
                return true;
            }
            if s.active_tool.is_retouch_or_mask() {
                s.set_canvas_tool_tx(CanvasTool::Hand, tx);
                return true;
            }
            if s.camera.crop_mode {
                s.toggle_crop_mode_tx(tx);
                return true;
            }
            false
        }
        Key::Text(ch) if cmd && ch == "," => {
            let next = !s.show_settings;
            s.set_show_settings_tx(next, tx);
            true
        }
        Key::Text(ch) if cmd && ev.mods.shift && ch.eq_ignore_ascii_case("e") => {
            let next = !s.show_export;
            s.set_show_export_tx(next, tx);
            true
        }
        Key::Text(ch) if cmd && ev.mods.alt && ch.eq_ignore_ascii_case("e") => {
            trigger_export_dialog(s, tx);
            true
        }
        _ if s.show_about
            || s.confirm_close
            || s.show_settings
            || s.show_export
            || s.any_text_input_focused() =>
        {
            false
        }
        Key::Enter if cmd => {
            s.toggle_compare_variant_tx(tx);
            true
        }
        Key::Left if !cmd => {
            s.select_prev_photo_tx(tx);
            true
        }
        Key::Right if !cmd => {
            s.select_next_photo_tx(tx);
            true
        }
        Key::Text(ch)
            if !cmd && !ev.mods.alt && s.shortcuts.matches(ShortcutAction::CompareSplit, ch) =>
        {
            s.camera.split_compare = !s.camera.split_compare;
            s.sync_window_state(tx);
            true
        }
        Key::Text(ch)
            if !cmd && !ev.mods.alt && s.shortcuts.matches(ShortcutAction::GridBrowser, ch) =>
        {
            s.toggle_grid_browser_tx(tx);
            true
        }
        Key::Text(ch) if !cmd && !ev.mods.alt && s.shortcuts.matches(ShortcutAction::Brush, ch) => {
            s.set_canvas_tool_tx(CanvasTool::Brush, tx);
            true
        }
        Key::Text(ch)
            if !cmd && !ev.mods.alt && s.shortcuts.matches(ShortcutAction::LinearMask, ch) =>
        {
            s.set_canvas_tool_tx(CanvasTool::LinearGradient, tx);
            true
        }
        Key::Text(ch) if !cmd && ev.mods.alt && ch.eq_ignore_ascii_case("l") => {
            s.toggle_loupe_tx(tx);
            true
        }
        Key::Text(ch) if !cmd && !ev.mods.alt && s.shortcuts.matches(ShortcutAction::Loupe, ch) => {
            s.toggle_loupe_tx(tx);
            true
        }
        Key::Text(ch)
            if !cmd && !ev.mods.alt && s.shortcuts.matches(ShortcutAction::RedMaskOverlay, ch) =>
        {
            s.toggle_red_mask_overlay_tx(tx);
            true
        }
        Key::Text(ch) if !cmd && ev.mods.alt && ch.eq_ignore_ascii_case("m") => {
            s.toggle_gray_mask_overlay_tx(tx);
            true
        }
        Key::Text(ch)
            if !cmd
                && !ev.mods.alt
                && (s.active_tab == InspectorTab::Layers || s.active_tool.is_retouch_or_mask())
                && s.shortcuts.matches(ShortcutAction::Eraser, ch) =>
        {
            s.set_canvas_tool_tx(CanvasTool::Eraser, tx);
            true
        }
        Key::Text(ch)
            if !cmd
                && !ev.mods.alt
                && (s.active_tab == InspectorTab::Layers || s.active_tool.is_retouch_or_mask())
                && s.shortcuts.matches(ShortcutAction::RadialMask, ch) =>
        {
            s.set_canvas_tool_tx(CanvasTool::RadialGradient, tx);
            true
        }
        Key::Text(ch)
            if !cmd
                && !ev.mods.alt
                && (s.active_tab == InspectorTab::Layers || s.active_tool.is_retouch_or_mask())
                && s.shortcuts.matches(ShortcutAction::Heal, ch) =>
        {
            s.set_canvas_tool_tx(CanvasTool::Heal, tx);
            true
        }
        Key::Text(ch)
            if !cmd
                && !ev.mods.alt
                && (s.active_tab == InspectorTab::Layers || s.active_tool.is_retouch_or_mask())
                && s.shortcuts.matches(ShortcutAction::Clone, ch) =>
        {
            s.set_canvas_tool_tx(CanvasTool::Clone, tx);
            true
        }
        Key::Text(ch)
            if !cmd && !ev.mods.alt && s.shortcuts.matches(ShortcutAction::Filmstrip, ch) =>
        {
            s.show_filmstrip = !s.show_filmstrip;
            s.sync_window_state(tx);
            true
        }
        Key::Text(ch)
            if !cmd && !ev.mods.alt && s.shortcuts.matches(ShortcutAction::Inspector, ch) =>
        {
            s.show_inspector = !s.show_inspector;
            s.sync_window_state(tx);
            true
        }
        Key::Text(ch) if !cmd && !ev.mods.alt && s.shortcuts.matches(ShortcutAction::Crop, ch) => {
            s.toggle_crop_mode_tx(tx);
            true
        }
        Key::Text(ch)
            if !cmd && !ev.mods.alt && s.shortcuts.matches(ShortcutAction::PickFlag, ch) =>
        {
            s.toggle_active_flag_tx(CullFlag::Pick, tx);
            true
        }
        Key::Text(ch)
            if !cmd && !ev.mods.alt && s.shortcuts.matches(ShortcutAction::RejectFlag, ch) =>
        {
            s.toggle_active_flag_tx(CullFlag::Reject, tx);
            true
        }
        Key::Text(ch)
            if !cmd
                && ev.mods.alt
                && matches!(ch.as_str(), "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7") =>
        {
            let tag = match ch.as_str() {
                "1" => ColorTag::Red,
                "2" => ColorTag::Orange,
                "3" => ColorTag::Yellow,
                "4" => ColorTag::Green,
                "5" => ColorTag::Blue,
                "6" => ColorTag::Purple,
                "7" => ColorTag::Pink,
                _ => ColorTag::None,
            };
            s.set_active_color_tag_tx(tag, tx);
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
        Key::Text(ch) if cmd && !ev.mods.shift && ch.eq_ignore_ascii_case("e") => {
            s.toggle_exposure_warning_tx(tx);
            true
        }
        Key::Text(ch) if cmd && ev.mods.shift && ch.eq_ignore_ascii_case("m") => {
            s.cycle_proof_margin_tx(tx);
            true
        }
        Key::Text(ch) if cmd && !ev.mods.shift && ch.eq_ignore_ascii_case("m") => {
            s.toggle_focus_mask_tx(tx);
            true
        }
        Key::Text(ch) if cmd && !ev.mods.shift && ch.eq_ignore_ascii_case("j") => {
            s.move_active_to_selects_tx(tx);
            true
        }
        Key::Text(ch) if cmd && ev.mods.shift && ch.eq_ignore_ascii_case("w") => {
            s.edit_with_external_editor_tx(tx);
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
            s.paste_selected_adjustments_tx(tx);
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

/// Slim Capture One-style Variant Strip rendered above the viewport when multiple variants
/// exist or Compare Variant mode is active.
fn variant_strip_bar(s: &AppState, k: &Kit) -> El {
    let v_count = s.active_variant_count();
    let v_active = s.active_variant_index();
    let mut left = row().items_center().gap(space::XS).child(
        text("VARIANTS")
            .font(ty::section_header(k))
            .color(k.c.text_faint),
    );
    for idx in 0..v_count.min(6) {
        let active = idx == v_active;
        let label = format!("V{}", idx + 1);
        left = left.child(
            row()
                .key(format!("vp-variant-{idx}"))
                .role("radio")
                .label(format!("Variant {label}"))
                .checked(active)
                .cursor(Cursor::Pointer)
                .h(size::PILL_COMPACT_H)
                .px(space::MD)
                .center()
                .radius(k.radius.sm)
                .fill(color::pill_fill(k, active))
                .hover(k.c.hover)
                .border(
                    if active {
                        border::REGULAR
                    } else {
                        border::HAIRLINE
                    },
                    color::pill_border(k, active),
                )
                .child(text(label).font(ty::mono_compact(k)).color(if active {
                    k.c.text_accent
                } else {
                    k.c.text_muted
                }))
                .on_press_tx(move |s: &mut AppState, tx| s.select_variant_tx(idx, tx)),
        );
    }
    left = left
        .child(
            Button::new("+ New")
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .on_press_tx(|s: &mut AppState, tx| s.new_variant_tx(None, tx))
                .build(k),
        )
        .child(
            Button::new("Clone")
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .on_press_tx(|s: &mut AppState, tx| s.clone_variant_tx(None, tx))
                .build(k),
        );

    let cmp_btn = if s.compare_reference.is_some() {
        Button::new("Clear Compare")
            .variant(ButtonVariant::Neutral)
            .control_size(ControlSize::Small)
            .shortcut("cmd+enter")
            .on_press_tx(|s: &mut AppState, tx| s.clear_compare_variant_tx(tx))
            .build(k)
    } else {
        Button::new("Compare Variant")
            .variant(ButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .shortcut("cmd+enter")
            .on_press_tx(|s: &mut AppState, tx| s.toggle_compare_variant_tx(tx))
            .build(k)
    };

    row()
        .key("variant-strip-bar")
        .w_full()
        .h(size::VARIANT_STRIP_H)
        .px(space::XL)
        .items_center()
        .justify_between()
        .fill(k.c.bg_deep)
        .border_sides(
            [border::NONE, border::NONE, border::REGULAR, border::NONE],
            k.c.border_muted,
        )
        .child(left)
        .child(cmp_btn)
}

/// Capture One Pro iconic orange-bordered Compare Variant Reference pane (`#E87A2A`).
fn compare_reference_panel(cmp: &CompareReference, k: &Kit) -> El {
    let ref_badge = k.elevate(
        row()
            .key("hud-compare-ref")
            .pin(Some(space::HUD_X), Some(space::HUD_Y), None, None)
            .items_center()
            .gap(space::SM)
            .px(space::LG)
            .py(space::TINY)
            .radius(k.radius.md)
            .continuous_corners()
            .backdrop_blur(size::HUD_BLUR)
            .fill(color::compare_reference_bg())
            .border(border::REGULAR, color::compare_reference_border())
            .child(
                text(format!("REFERENCE · {} · {}", cmp.variant_tag, cmp.label))
                    .font(ty::section_header(k))
                    .color(color::compare_reference_border()),
            ),
        &k.elevation.floating,
    );

    el().key("compare-reference-pane")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .padding(space::XXL)
        .fill(k.c.page)
        .border(border::DROP_TARGET, color::compare_reference_border())
        .child(
            el().w_full()
                .h_full()
                .pixels_fit(&cmp.pixels, SurfaceFit::Fit),
        )
        .child(ref_badge)
}

fn view(s: &AppState, cx: &mut Cx<AppState>) -> El {
    let k = tokens::kit();
    let insets = cx.chrome_insets();

    let body = if s.show_settings {
        settings_view(s, k)
    } else if s.show_export {
        export_view(s, k)
    } else {
        let mut stage_col = col()
            .key("stage-column")
            .grow(1.0)
            .h_full()
            .min_w(0.0)
            .min_h(0.0);

        if s.show_grid_browser {
            stage_col = stage_col.child(grid_browser_view(s, k));
        } else {
            if s.active_variant_count() > 1 || s.compare_reference.is_some() {
                stage_col = stage_col.child(variant_strip_bar(s, k));
            }
            if let Some(ref cmp) = s.compare_reference {
                let split_stage = row()
                    .key("compare-variant-stage")
                    .grow(1.0)
                    .w_full()
                    .min_h(0.0)
                    .gap(space::XS)
                    .fill(k.c.page)
                    .child(compare_reference_panel(cmp, k))
                    .child(photo_viewport(s, k));
                stage_col = stage_col.child(split_stage);
            } else {
                stage_col = stage_col.child(photo_viewport(s, k));
            }
            if s.show_filmstrip {
                stage_col = stage_col.child(filmstrip_bar(s, k));
            }
        }

        let mut workspace = row()
            .key("workspace-body")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .child(stage_col);

        if s.show_inspector {
            workspace = workspace.child(right_inspector(s, k));
        }
        workspace
    };

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
    if s.overlays_popover_open {
        overlays.push(overlays_popover_overlay(s, k));
    }
    if let Some(info_card) = floating_info_popover(s, k) {
        overlays.push(info_card);
    }
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
    s.base_open = true;
    s.wb_open = true;
    s.tone_open = true;
    s.levels_open = false;
    s.curve_open = false;
    s.recipe.film_curve = FilmCurve::FilmExtraShadow;
    s.recipe.apply_wb_preset(WbPreset::Daylight);
    s.recipe.exposure = 0.12;
    s.recipe.contrast = 0.14;
    s.recipe.highlights = -0.18;
    s.recipe.shadows = 0.12;
    s.recipe.whites = 0.06;
    s.recipe.blacks = -0.05;
    s.recipe.vibrance = 0.10;
    s.recipe.vignette = -0.08;
    s.recipe.preset_name = None;
    s.status_message = Some("Subtle editorial grade • Film Extra Shadow • Daylight".to_owned());
    s.recompute_histogram();
}

fn scene_color(s: &mut AppState) {
    s.active_tab = InspectorTab::Color;
    s.color_balance_open = true;
    s.presence_open = true;
    s.hsl_open = true;
    s.color_balance_view = ColorBalanceView::ThreeWay;
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

fn scene_color_master(s: &mut AppState) {
    scene_color(s);
    s.color_balance_view = ColorBalanceView::Master;
    s.recipe.master_wheel = ColorWheel::new(0.26, 0.14, 0.06);
    s.status_message = Some("Master Color Balance Wheel • Global Grade".to_owned());
    s.recompute_histogram();
}

fn scene_tone_curve(s: &mut AppState) {
    s.active_tab = InspectorTab::Light;
    s.base_open = false;
    s.wb_open = false;
    s.tone_open = false;
    s.levels_open = false;
    s.curve_open = true;
    s.curve_channel = CurveChannel::Rgb;
    s.recipe.exposure = 0.20;
    s.recipe.contrast = 0.15;
    s.recipe.highlights = -0.35;
    s.recipe.shadows = 0.25;
    s.recipe.curves = ToneCurves {
        rgb: vec![[0.0, 0.02], [0.28, 0.22], [0.72, 0.78], [1.0, 0.98]],
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
    s.cursor_readout = Some(CursorColorReadout {
        rgb8: [198, 162, 134],
        hsl: [26.0, 41.0, 65.0],
        lab: [69.0, 10.0, 19.0],
    });
    s.status_message = Some("5-Tab Tone Curve (RGB + Luma) • Live Tonal Indicator".to_owned());
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
    s.base_open = false;
    s.wb_open = false;
    s.tone_open = false;
    s.levels_open = true;
    s.levels_advanced_open = true;
    s.curve_open = false;
    s.levels_channel = LevelsChannel::Red;
    s.recipe.brightness = 0.16;
    s.recipe.contrast = 0.12;
    s.recipe.levels = auto_levels(&s.image.histogram_proxy);
    s.recipe.levels.red.in_black = 0.04;
    s.recipe.levels.red.in_mid = 0.14;
    s.recipe.levels.red.in_white = 0.96;
    s.recipe.levels.blue.in_mid = -0.10;
    s.recipe.preset_name = None;
    s.status_message = Some("Per-Channel Levels (RGB / R / G / B) • Auto 0.1% clip".to_owned());
    s.recompute_histogram();
}

fn scene_bw(s: &mut AppState) {
    s.active_tab = InspectorTab::Color;
    s.color_balance_open = false;
    s.presence_open = false;
    s.bw_open = true;
    s.bw_split_open = true;
    s.hsl_open = false;
    s.recipe.contrast = 0.24;
    s.recipe.clarity = 0.18;
    s.recipe.structure = 0.22;
    s.recipe.bw.enabled = true;
    s.recipe.bw.red = 0.34;
    s.recipe.bw.yellow = 0.20;
    s.recipe.bw.cyan = -0.25;
    s.recipe.bw.blue = -0.45;
    s.recipe.bw.split_shadow_hue = 215.0;
    s.recipe.bw.split_shadow_sat = 0.24;
    s.recipe.bw.split_highlight_hue = 38.0;
    s.recipe.bw.split_highlight_sat = 0.18;
    s.recipe.preset_name = None;
    s.status_message = Some("6-Channel Black & White + Split Tones".to_owned());
    s.recompute_histogram();
}

fn scene_detail(s: &mut AppState) {
    s.active_tab = InspectorTab::Detail;
    s.clarity_open = true;
    s.clarity_advanced_open = true;
    s.sharpen_open = true;
    s.sharpen_advanced_open = true;
    s.nr_open = true;
    s.grain_open = true;
    s.vignette_open = true;
    s.recipe.clarity_method = ClarityMethod::Punch;
    s.recipe.clarity = 0.22;
    s.recipe.structure = 0.18;
    s.recipe.dehaze = 0.10;
    s.recipe.sharpen = 0.34;
    s.recipe.sharpen_radius = 1.4;
    s.recipe.sharpen_threshold = 0.12;
    s.recipe.sharpen_halo = 0.28;
    s.recipe.noise_reduction = 0.18;
    s.recipe.nr_color = 0.32;
    s.recipe.grain_type = GrainType::SilverRich;
    s.recipe.grain = 0.24;
    s.recipe.grain_size = 0.48;
    s.recipe.vignette = -0.14;
    s.recipe.preset_name = None;
    s.status_message = Some("Clarity Method • Sharpening • NR • Silver Rich Grain".to_owned());
    s.recompute_histogram();
}

fn scene_grain(s: &mut AppState) {
    scene_detail(s);
    s.clarity_open = false;
    s.sharpen_open = false;
    s.nr_open = true;
    s.grain_open = true;
    s.grain_advanced_open = true;
    s.vignette_open = true;
    s.vignette_advanced_open = true;
    s.recipe.vignette_method = VignetteMethod::CircularOnCrop;
    s.status_message =
        Some("Noise Reduction • Film Grain Profiles • Crop-Aware Vignetting".to_owned());
}

fn scene_color_advanced(s: &mut AppState) {
    s.active_tab = InspectorTab::Color;
    s.color_balance_open = false;
    s.presence_open = false;
    s.bw_open = false;
    s.hsl_open = true;
    s.color_adv_range_open = true;
    s.color_editor_mode = ColorEditorMode::Advanced;
    s.selected_color_slice = 0;
    s.recipe.advanced_color[0].enabled = true;
    s.recipe.advanced_color[0].target_hue = 24.0;
    s.recipe.advanced_color[0].target_sat = 0.56;
    s.recipe.advanced_color[0].hue_range = 36.0;
    s.recipe.advanced_color[0].sat_range = 0.50;
    s.recipe.advanced_color[0].smoothness = 0.45;
    s.recipe.advanced_color[0].uniformity = 0.35;
    s.recipe.advanced_color[0].hue_shift = -0.18;
    s.recipe.advanced_color[0].saturation = 0.24;
    s.recipe.advanced_color[0].lightness = 0.08;
    s.recipe.advanced_color[1].enabled = true;
    s.recipe.advanced_color[1].target_hue = 135.0;
    s.recipe.advanced_color[1].target_sat = 0.42;
    s.recipe.advanced_color[1].saturation = -0.15;
    s.recipe.preset_name = None;
    s.status_message = Some("Advanced Color Editor • Custom Slice 1 & Uniformity".to_owned());
    s.recompute_histogram();
}

fn scene_skin_tone(s: &mut AppState) {
    s.active_tab = InspectorTab::Color;
    s.color_balance_open = false;
    s.presence_open = false;
    s.bw_open = false;
    s.hsl_open = true;
    s.skin_tone_advanced_open = true;
    s.color_editor_mode = ColorEditorMode::SkinTone;
    s.recipe.skin_tone.enabled = true;
    s.recipe.skin_tone.target_hue = 22.0;
    s.recipe.skin_tone.target_sat = 0.44;
    s.recipe.skin_tone.target_lightness = 0.64;
    s.recipe.skin_tone.smoothness = 0.50;
    s.recipe.skin_tone.uniformity_hue = 0.45;
    s.recipe.skin_tone.uniformity_sat = 0.32;
    s.recipe.skin_tone.uniformity_lightness = 0.20;
    s.recipe.skin_tone.amount_hue = 0.08;
    s.recipe.skin_tone.amount_sat = -0.06;
    s.recipe.skin_tone.amount_lightness = 0.10;
    s.recipe.preset_name = None;
    s.status_message = Some("Skin Tone Uniformity • Target Hue/Sat/Lightness".to_owned());
    s.recompute_histogram();
}

fn scene_lens_keystone(s: &mut AppState) {
    s.active_tab = InspectorTab::Crop;
    s.crop_open = false;
    s.rotate_open = false;
    s.keystone_open = true;
    s.keystone_advanced_open = true;
    s.lens_open = true;
    s.lens_advanced_open = true;
    s.grid_open = false;
    s.recipe.geometry.keystone_vertical = -0.16;
    s.recipe.geometry.keystone_horizontal = 0.08;
    s.recipe.geometry.keystone_aspect = 0.10;
    s.recipe.geometry.keystone_amount = 0.80;
    s.recipe.lens.distortion = -0.14;
    s.recipe.lens.chromatic_aberration = 0.18;
    s.recipe.lens.purple_fringing = 0.25;
    s.recipe.lens.light_falloff = 0.32;
    s.recipe.preset_name = None;
    s.status_message = Some("Keystone Homography & Optical Lens Correction".to_owned());
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

fn scene_settings(s: &mut AppState) {
    s.show_settings = true;
    s.settings_section = SettingsSection::AgentMcp;
    s.settings_mcp_snippets_open = true;
    s.settings_mcp_tools_open = false;
    s.mcp_request_count = 14;
    s.mcp_last_activity = Some("rune_edit_photo • ok".to_owned());
    s.status_message = Some("Settings — Local MCP Server (2025-11-25)".to_owned());
}

fn scene_grid_browser(s: &mut AppState) {
    scene_editorial(s);
    if let Some(meta) = s.catalog.entries.get_mut(0).map(|e| &mut e.meta) {
        meta.clone_variant(Some("Warm Study".to_owned()));
        meta.select_variant(0);
    }
    s.show_grid_browser = true;
    s.auto_advance = true;
    s.active_tab = InspectorTab::Light;
    s.status_message = Some("Grid Browser • 7 Color Tags • Variants & Auto-Advance".to_owned());
}

fn scene_compare_variant(s: &mut AppState) {
    scene_editorial(s);
    s.pin_compare_variant();
    s.clone_variant(Some("Editorial B&W".to_owned()));
    s.apply_preset("editorial_bw");
    s.active_tab = InspectorTab::Presets;
    s.status_message = Some("Compare Variant • Reference V1 (Orange) vs. Candidate V2".to_owned());
}

fn scene_clipboard_styles(s: &mut AppState) {
    scene_editorial(s);
    s.active_tab = InspectorTab::Presets;
    s.stack_styles = true;
    s.variants_open = false;
    s.styles_open = false;
    s.style_strength = 0.75;
    s.user_styles_open = true;
    s.user_styles_advanced_open = true;
    s.clipboard_open = true;
    s.clipboard_groups_open = true;
    s.catalog.copy_adjustments(&s.recipe);
    s.autoselect_adjusted_groups();
    if s.user_styles.is_empty()
        && let Ok(demo_style) = UserStyle::from_recipe(
            "Studio Warm Portrait",
            "Warm highlights + soft shadows",
            &s.recipe,
            &["wb".to_owned(), "tone".to_owned(), "color".to_owned()],
        )
    {
        s.user_styles.push(demo_style);
    }
    s.status_message = Some("Styles, Stacking & Selective Adjustments Clipboard".to_owned());
}

fn scene_export_recipes(s: &mut AppState) {
    scene_editorial(s);
    s.show_export = true;
    s.export_watermark_enabled = true;
    s.export_watermark_opacity = 0.42;
    s.export_watermark_position = WatermarkPosition::BottomRight;
    s.export_color_space = ExportColorSpace::DisplayP3;
    s.export_strip_metadata = false;
    s.status_message =
        Some("Multi-Recipe Export • Dynamic Naming, Watermark & Display P3".to_owned());
}

fn scene_loupe(s: &mut AppState) {
    scene_editorial(s);
    s.camera.loupe_enabled = true;
    s.camera.loupe_uv = [0.49, 0.35];
    s.camera.loupe_zoom = 2.5;
    s.camera.loupe_radius_px = 96.0;
    s.pinned_readouts = vec![
        PinnedReadout {
            source_uv: [0.46, 0.38],
            slot_uv: [0.56, 0.44],
        },
        PinnedReadout {
            source_uv: [0.33, 0.50],
            slot_uv: [0.39, 0.50],
        },
        PinnedReadout {
            source_uv: [0.62, 0.78],
            slot_uv: [0.58, 0.76],
        },
    ];
    s.status_message = Some("120Hz GPU Loupe (2.5×) • 3 Pinned Color Readout Markers".to_owned());
}

fn scene_layers_mask(s: &mut AppState) {
    scene_editorial(s);
    s.active_tab = InspectorTab::Layers;
    s.layers_open = true;
    s.layers_mask_actions_open = true;
    s.masks_open = true;
    s.local_adj_open = true;
    s.heal_open = false;
    let mut sky_layer =
        AdjustmentLayer::new("layer-1".to_owned(), "Sky & Portrait Glow".to_owned());
    sky_layer.opacity = 0.85;
    sky_layer.masks = vec![
        LayerMaskItem::new(MaskShape::LinearGradient {
            start: [0.50, 0.14],
            end: [0.50, 0.62],
            feather: 0.72,
        }),
        LayerMaskItem {
            enabled: true,
            op: MaskCombineOp::Subtract,
            inverted: false,
            shape: MaskShape::RadialGradient {
                center: [0.50, 0.42],
                radius: [0.22, 0.28],
                angle_deg: 0.0,
                feather: 0.48,
            },
        },
    ];
    sky_layer.adjustments = LocalAdjustments {
        exposure: -0.45,
        contrast: 0.18,
        highlights: -0.25,
        temperature: -0.14,
        clarity: 0.16,
        ..LocalAdjustments::default()
    };
    let mut ai_layer = AdjustmentLayer::new_with_mask(
        "layer-2".to_owned(),
        "AI Subject Pop".to_owned(),
        rune_editor_decode::generate_subject_mask(&s.image, false),
    );
    ai_layer.adjustments = LocalAdjustments {
        exposure: 0.22,
        clarity: 0.14,
        sharpen: 0.25,
        ..LocalAdjustments::default()
    };
    s.recipe.layers = vec![sky_layer, ai_layer];
    s.active_layer_index = Some(0);
    s.camera.active_layer_index = 0;
    s.camera.mask_overlay_mode = 1;
    s.active_tool = CanvasTool::LinearGradient;
    s.status_message = Some("Layers • Linear − Radial Combined Mask • Red Overlay (M)".to_owned());
    s.recompute_histogram();
}

fn scene_layers_grayscale(s: &mut AppState) {
    scene_editorial(s);
    s.active_tab = InspectorTab::Layers;
    s.layers_open = true;
    s.layers_mask_actions_open = true;
    s.masks_open = true;
    s.local_adj_open = false;
    s.heal_open = false;
    let mut luma_layer = AdjustmentLayer::new_with_mask(
        "layer-1".to_owned(),
        "Highlight Luma Range".to_owned(),
        MaskShape::LumaRange {
            low: 0.42,
            high: 1.0,
            low_falloff: 0.18,
            high_falloff: 0.05,
        },
    );
    luma_layer.adjustments = LocalAdjustments {
        highlights: -0.38,
        temperature: 0.16,
        clarity: 0.12,
        ..LocalAdjustments::default()
    };
    s.recipe.layers = vec![luma_layer];
    s.active_layer_index = Some(0);
    s.camera.active_layer_index = 0;
    s.camera.mask_overlay_mode = 2;
    s.status_message = Some("Layers • Luma Range Mask • Grayscale View (⌥M)".to_owned());
    s.recompute_histogram();
}

fn scene_heal_clone(s: &mut AppState) {
    scene_editorial(s);
    s.active_tab = InspectorTab::Layers;
    s.layers_open = false;
    s.masks_open = false;
    s.local_adj_open = false;
    s.heal_open = true;
    s.heal_advanced_open = true;
    s.active_tool = CanvasTool::Heal;
    s.recipe.heal_spots = vec![
        HealSpot {
            id: "heal-1".to_owned(),
            target: [0.36, 0.34],
            source: [0.24, 0.30],
            radius: 0.038,
            feather: 0.45,
            opacity: 1.0,
            mode: HealMode::Heal,
        },
        HealSpot {
            id: "heal-2".to_owned(),
            target: [0.64, 0.46],
            source: [0.76, 0.42],
            radius: 0.034,
            feather: 0.40,
            opacity: 0.95,
            mode: HealMode::Heal,
        },
        HealSpot {
            id: "heal-3".to_owned(),
            target: [0.58, 0.70],
            source: [0.72, 0.72],
            radius: 0.042,
            feather: 0.35,
            opacity: 0.90,
            mode: HealMode::Clone,
        },
    ];
    s.status_message = Some("Spot Heal & Clone • 3 Retouch Pins (Target → Source)".to_owned());
    s.recompute_histogram();
}

fn scene_film_negative(s: &mut AppState) {
    s.active_tab = InspectorTab::Light;
    s.base_open = true;
    s.film_neg_open = true;
    s.film_neg_advanced_open = true;
    s.wb_open = false;
    s.tone_open = false;
    s.levels_open = false;
    s.curve_open = false;
    s.recipe.processing_mode = ProcessingMode::FilmNegative;
    s.recipe.film_negative = FilmNegativeParams {
        base_r: 0.94,
        base_g: 0.56,
        base_b: 0.34,
        density: 0.18,
        contrast: 0.24,
        black_clip: 0.03,
        white_clip: 0.96,
    };
    s.recipe.preset_name = None;
    s.status_message =
        Some("Film Negative Conversion • C-41 Orange Mask Base Calibration".to_owned());
    s.recompute_histogram();
}

fn scene_camera_profiles(s: &mut AppState) {
    s.active_tab = InspectorTab::Light;
    s.base_open = true;
    s.profile_picker_open = true;
    s.base_advanced_open = false;
    s.wb_open = false;
    s.tone_open = false;
    s.levels_open = false;
    s.curve_open = false;
    s.recipe.camera_profile = "classic_chrome".to_owned();
    s.recipe.profile_strength = 0.85;
    s.recipe.film_curve = FilmCurve::FilmStandard;
    s.recipe.preset_name = None;
    s.status_message = Some("Camera Profile: Classic Chrome (85%) • 33³ Linear 3D LUT".to_owned());
    s.recompute_histogram();
}

fn scene_proof_margin(s: &mut AppState) {
    scene_editorial(s);
    s.proof_margin = ProofMargin::Exhibition;
    s.status_message = Some("Proof Margin: Exhibition (84 pt Gallery Mat • ⌘⇧M)".to_owned());
}

fn scene_settings_shortcuts(s: &mut AppState) {
    s.show_settings = true;
    s.settings_section = SettingsSection::Shortcuts;
    s.proof_margin = ProofMargin::Studio;
    s.hot_folder_style = Some("warm_film".to_owned());
    s.hot_folder_dir = Some(std::path::PathBuf::from(
        "/Users/studio/Sessions/Editorial/Capture",
    ));
    let _ = s.shortcuts.set_key(ShortcutAction::Loupe, "v");
    s.status_message = Some("Settings — Keyboard Shortcuts & Hot Folder Auto-Ingest".to_owned());
}

fn main() {
    let spec = AppSpec::<AppState> {
        min_size: Some(size::WINDOW_MIN),
        fonts: rune_theme_opencode::FONTS,
        families: rune_theme_opencode::FAMILIES,
        scenes: &[
            ("editorial", scene_editorial),
            ("color", scene_color),
            ("color-master", scene_color_master),
            ("color-advanced", scene_color_advanced),
            ("skin-tone", scene_skin_tone),
            ("tone-curve", scene_tone_curve),
            ("split", scene_split),
            ("filmstrip", scene_filmstrip),
            ("grid-browser", scene_grid_browser),
            ("compare-variant", scene_compare_variant),
            ("clipboard-styles", scene_clipboard_styles),
            ("export-recipes", scene_export_recipes),
            ("loupe", scene_loupe),
            ("layers-mask", scene_layers_mask),
            ("layers-grayscale", scene_layers_grayscale),
            ("heal-clone", scene_heal_clone),
            ("film-negative", scene_film_negative),
            ("camera-profiles", scene_camera_profiles),
            ("proof-margin", scene_proof_margin),
            ("settings-shortcuts", scene_settings_shortcuts),
            ("crop", scene_crop),
            ("lens-keystone", scene_lens_keystone),
            ("levels", scene_levels),
            ("bw", scene_bw),
            ("detail", scene_detail),
            ("grain", scene_grain),
            ("warnings", scene_warnings),
            ("settings", scene_settings),
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
    let headless_snap = std::env::args().any(|a| a == "--snap" || a.starts_with("--snap="));
    let has_scene = std::env::args().any(|a| a == "--scene" || a.starts_with("--scene="));
    app.init(move || {
        initial_state.app_handle = Some(handle.clone());
        if !headless_snap {
            initial_state.init_mcp_if_enabled();
        }
        if !headless_snap
            && !has_scene
            && let Some(target_path) = rune::tooling::extra_args()
                .iter()
                .filter(|arg| !arg.starts_with('-'))
                .map(std::path::PathBuf::from)
                .find(|p| p.exists())
        {
            handle.update_if(move |s, tx| {
                if target_path.is_dir() {
                    s.open_folder_tx(target_path, tx);
                } else {
                    s.open_image_path_tx(target_path, tx);
                }
                true
            });
        }
        initial_state
    })
    .run();
}

#[cfg(test)]
mod scroll_tests {
    use super::*;
    use rune::{Env, Runtime, ScrollBars};

    fn find_rect(rt: &Runtime<AppState>, key: &str) -> [f32; 4] {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key))
            .unwrap_or_else(|| panic!("missing element with key {key:?}"))
            .rect
    }

    fn find_first_child_rect(rt: &Runtime<AppState>, parent_key: &str) -> [f32; 4] {
        let elems = rt.elements();
        let parent_path = elems
            .iter()
            .find(|e| e.key.as_deref() == Some(parent_key))
            .unwrap_or_else(|| panic!("missing element with key {parent_key:?}"))
            .path
            .clone();
        let prefix = format!("{parent_path}/");
        elems
            .into_iter()
            .find(|e| {
                e.path
                    .strip_prefix(&prefix)
                    .is_some_and(|rest| !rest.contains('/'))
            })
            .unwrap_or_else(|| panic!("missing direct child of {parent_key:?}"))
            .rect
    }

    fn make_runtime(state: AppState, size: (f32, f32)) -> Runtime<AppState> {
        let mut rt = Runtime::new(state, view, size, Env::new(true, 1.0));
        rt.set_system_scroll_bars(ScrollBars::Always);
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        rt
    }

    #[test]
    fn right_inspector_scrolls_vertically_across_tabs_and_reaches_bottom() {
        type TabSetup = (&'static str, fn(&mut AppState));
        let tabs: &[TabSetup] = &[
            ("Light", |s| {
                scene_editorial(s);
                s.active_tab = InspectorTab::Light;
                s.levels_open = true;
                s.curve_open = true;
            }),
            ("Color", |s| {
                scene_color(s);
                s.active_tab = InspectorTab::Color;
                s.color_balance_open = true;
                s.hsl_open = true;
                s.bw_open = true;
            }),
            ("Detail", |s| {
                scene_detail(s);
                s.active_tab = InspectorTab::Detail;
                s.clarity_open = true;
                s.sharpen_open = true;
                s.grain_open = true;
                s.vignette_open = true;
            }),
            ("Crop/Lens", |s| {
                scene_lens_keystone(s);
                s.active_tab = InspectorTab::Crop;
                s.crop_open = true;
                s.rotate_open = true;
                s.keystone_open = true;
                s.lens_open = true;
                s.grid_open = true;
            }),
            ("Layers", |s| {
                scene_layers_mask(s);
                s.heal_open = true;
            }),
            ("Style", |s| {
                scene_clipboard_styles(s);
                s.variants_open = true;
                s.styles_open = true;
                s.user_styles_open = true;
                s.clipboard_open = true;
            }),
        ];

        for &(label, setup) in tabs {
            let mut state = AppState::default();
            setup(&mut state);
            let mut rt = make_runtime(state, size::WINDOW_DEFAULT);

            let vp = find_rect(&rt, "inspector-scroll");
            let content_before = find_rect(&rt, "inspector-tabpanel");
            let bar = find_rect(&rt, "scroll-bar");

            assert!(
                content_before[3] > vp[3] + 20.0,
                "{label} tab content ({}) must exceed inspector-scroll viewport ({}) rather than being clamped",
                content_before[3],
                vp[3],
            );
            assert!(
                bar[3] > 100.0,
                "{label} tab scroll-bar must be visible with non-zero height, got {bar:?}",
            );

            let cx = vp[0] + vp[2] * 0.5;
            let cy = vp[1] + vp[3] * 0.5;
            let initial_zoom = rt.state.camera.zoom;

            // Scroll down in steps until the bottom of content is visible.
            for _ in 0..8 {
                rt.wheel_event(WheelEvent::new(0.0, -240.0).at(cx, cy));
                rt.frame(1.0 / 60.0);
            }
            for _ in 0..30 {
                rt.frame(1.0 / 60.0);
            }

            let content_after = find_rect(&rt, "inspector-tabpanel");
            assert!(
                content_after[1] < content_before[1] - 100.0,
                "{label} tab did not scroll on wheel: before={content_before:?} after={content_after:?}",
            );
            let content_bottom = content_after[1] + content_after[3];
            let vp_bottom = vp[1] + vp[3];
            assert!(
                content_bottom <= vp_bottom + 1.5,
                "{label} tab could not scroll all the way to bottom: content_bottom={content_bottom} vp_bottom={vp_bottom}",
            );
            assert_eq!(
                rt.state.camera.zoom, initial_zoom,
                "wheeling over {label} inspector must not affect viewport camera zoom",
            );
        }
    }

    #[test]
    fn settings_export_browser_and_filmstrip_scroll() {
        // 1. Settings pane vertical scroll (short window height).
        {
            let mut state = AppState::default();
            scene_settings_shortcuts(&mut state);
            let mut rt = make_runtime(state, size::WINDOW_MIN);
            let vp = find_rect(&rt, "settings-scroll");
            let before = find_first_child_rect(&rt, "settings-scroll");
            assert!(
                before[3] > vp[3] + 20.0,
                "settings content ({}) must exceed viewport ({})",
                before[3],
                vp[3],
            );
            rt.wheel_event(
                WheelEvent::new(0.0, -240.0).at(vp[0] + vp[2] * 0.5, vp[1] + vp[3] * 0.5),
            );
            for _ in 0..20 {
                rt.frame(1.0 / 60.0);
            }
            let after = find_first_child_rect(&rt, "settings-scroll");
            assert!(
                after[1] < before[1] - 50.0,
                "settings pane did not scroll: before={before:?} after={after:?}",
            );
        }

        // 2. Export pane vertical scroll (short window height).
        {
            let mut state = AppState::default();
            scene_export_recipes(&mut state);
            let mut rt = make_runtime(state, size::WINDOW_MIN);
            let vp = find_rect(&rt, "export-scroll");
            let before = find_first_child_rect(&rt, "export-scroll");
            assert!(
                before[3] > vp[3] + 20.0,
                "export content ({}) must exceed viewport ({})",
                before[3],
                vp[3],
            );
            rt.wheel_event(
                WheelEvent::new(0.0, -240.0).at(vp[0] + vp[2] * 0.5, vp[1] + vp[3] * 0.5),
            );
            for _ in 0..20 {
                rt.frame(1.0 / 60.0);
            }
            let after = find_first_child_rect(&rt, "export-scroll");
            assert!(
                after[1] < before[1] - 50.0,
                "export pane did not scroll: before={before:?} after={after:?}",
            );
        }

        // 3. Grid browser vertical scroll with >12 photos (4+ rows).
        {
            let mut state = AppState::default();
            scene_grid_browser(&mut state);
            while state.catalog.entries.len() < 16 {
                let mut dup = state.catalog.entries[0].clone();
                let n = state.catalog.entries.len();
                dup.path = std::path::PathBuf::from(format!("Frame_{n:03}.ARW"));
                state.catalog.entries.push(dup);
            }
            let mut rt = make_runtime(state, size::WINDOW_DEFAULT);
            let vp = find_rect(&rt, "grid-browser-scroll");
            let before = find_rect(&rt, "grid-browser-cards");
            assert!(
                before[3] > vp[3] + 20.0,
                "grid browser content ({}) must exceed viewport ({})",
                before[3],
                vp[3],
            );
            rt.wheel_event(
                WheelEvent::new(0.0, -240.0).at(vp[0] + vp[2] * 0.5, vp[1] + vp[3] * 0.5),
            );
            for _ in 0..20 {
                rt.frame(1.0 / 60.0);
            }
            let after = find_rect(&rt, "grid-browser-cards");
            assert!(
                after[1] < before[1] - 50.0,
                "grid browser did not scroll: before={before:?} after={after:?}",
            );
        }

        // 4. Filmstrip horizontal scroll with >10 photos.
        {
            let mut state = AppState::default();
            scene_filmstrip(&mut state);
            while state.catalog.entries.len() < 12 {
                let mut dup = state.catalog.entries[0].clone();
                let n = state.catalog.entries.len();
                dup.path = std::path::PathBuf::from(format!("Strip_{n:03}.ARW"));
                state.catalog.entries.push(dup);
            }
            let mut rt = make_runtime(state, size::WINDOW_DEFAULT);
            let vp = find_rect(&rt, "filmstrip-scroll");
            let before = find_rect(&rt, "filmstrip-items");
            assert!(
                before[2] > vp[2] + 50.0,
                "filmstrip items ({}) must exceed horizontal viewport ({})",
                before[2],
                vp[2],
            );
            rt.wheel_event(
                WheelEvent::new(-240.0, 0.0).at(vp[0] + vp[2] * 0.5, vp[1] + vp[3] * 0.5),
            );
            for _ in 0..20 {
                rt.frame(1.0 / 60.0);
            }
            let after = find_rect(&rt, "filmstrip-items");
            assert!(
                after[0] < before[0] - 50.0,
                "filmstrip did not scroll horizontally: before={before:?} after={after:?}",
            );
        }
    }

    #[test]
    fn studio_airy_overlays_and_info_popover_render_cleanly() {
        // 1. Overlays popover renders below titlebar and clears macOS traffic lights
        let s = AppState {
            overlays_popover_open: true,
            ..AppState::default()
        };
        let rt = make_runtime(s, size::WINDOW_DEFAULT);
        let inspector = find_rect(&rt, "develop-inspector");
        assert!(
            (inspector[2] - size::INSPECTOR_W).abs() < 1.0,
            "inspector width must match Studio Airy 388pt, got {:?}",
            inspector
        );
        let popover = find_rect(&rt, "overlays-popover-card");
        assert!(
            popover[1] >= size::TITLEBAR_H,
            "overlays popover must open below titlebar, got {popover:?}"
        );
        assert!(popover[2] > 200.0 && popover[3] > 120.0);

        // 2. Floating hover `ⓘ` info popover renders in Darkroom and Settings without clipping
        let tip = crate::state::InfoTip {
            title: "Exposure & Tone",
            shortcut: Some("Q / W / A / S"),
            description: "Scene-referred EV exposure and tonal recovery.",
            cli_key: "exposure  ·  contrast  ·  highlights  ·  shadows",
            tier: "Tier 0 · Tone",
        };
        let mut s_dark = AppState::default();
        s_dark.set_hovered_info(tip);
        let rt_dark = make_runtime(s_dark, size::WINDOW_DEFAULT);
        let card_dark = find_rect(&rt_dark, "floating-info-popover");
        assert!(card_dark[1] >= size::TITLEBAR_H);
        assert!(card_dark[0] + card_dark[2] <= size::WINDOW_DEFAULT.0 - size::INSPECTOR_W);

        let mut s_set = AppState::default();
        scene_settings(&mut s_set);
        s_set.set_hovered_info(tip);
        let rt_set = make_runtime(s_set, size::WINDOW_DEFAULT);
        let card_set = find_rect(&rt_set, "floating-info-popover");
        assert!(card_set[0] >= size::SETTINGS_SIDEBAR_W);
        assert!(card_set[1] + card_set[3] <= size::WINDOW_DEFAULT.1 - size::STATUS_BAR_H);
    }

    #[test]
    fn camera_profiles_scene_layout_and_tx_sync() {
        let mut s = AppState::default();
        scene_camera_profiles(&mut s);
        let mut rt = make_runtime(s, size::WINDOW_DEFAULT);

        let inspector = find_rect(&rt, "develop-inspector");
        let vp = find_rect(&rt, "inspector-scroll");
        let selector = find_rect(&rt, "camera-profile-selector");
        let strength = find_rect(&rt, "slider-profile-strength");
        let picker = find_rect(&rt, "camera-profile-picker");
        let cal_group = find_rect(&rt, "profile-group-calibration");
        let sim_group = find_rect(&rt, "profile-group-film-simulations");
        let user_group = find_rect(&rt, "profile-group-user");
        let import_btn = find_rect(&rt, "import-camera-profile-btn");
        let pro_std_btn = find_rect(&rt, "profile-opt-pro_standard");

        // All Camera Profile controls sit inside the right inspector, strictly below titlebar
        // (zero overlap on macOS traffic lights) and within the visible viewport.
        for (name, rect) in [
            ("camera-profile-selector", selector),
            ("slider-profile-strength", strength),
            ("camera-profile-picker", picker),
            ("profile-group-calibration", cal_group),
            ("profile-group-film-simulations", sim_group),
            ("profile-group-user", user_group),
            ("import-camera-profile-btn", import_btn),
            ("profile-opt-pro_standard", pro_std_btn),
        ] {
            assert!(
                rect[2] > 40.0 && rect[3] >= 20.0,
                "{name} must have non-zero size, got {rect:?}"
            );
            assert!(
                rect[0] >= inspector[0] - 0.5
                    && rect[0] + rect[2] <= inspector[0] + inspector[2] + 0.5,
                "{name} ({rect:?}) must stay within inspector horizontal bounds ({inspector:?})"
            );
            assert!(
                rect[1] >= size::TITLEBAR_H,
                "{name} ({rect:?}) must render strictly below titlebar / traffic lights"
            );
        }
        assert!(
            (strength[3] - size::CAPSULE_SLIDER_H).abs() < 1.0,
            "Profile Strength capsule must match Studio Airy 32pt height, got {strength:?}"
        );
        assert!(
            strength[1] + strength[3] <= vp[1] + vp[3] + 1.0,
            "Profile Strength capsule ({strength:?}) must be immediately visible inside viewport ({vp:?})"
        );
        assert!(
            import_btn[1] + import_btn[3] <= vp[1] + vp[3] + 1.0,
            "Import Profile button ({import_btn:?}) must be visible inside viewport ({vp:?})"
        );

        // Clicking `Rune ProStandard` invokes `set_camera_profile_tx`, pushes undo, and syncs MCP snapshot
        let click_x = pro_std_btn[0] + pro_std_btn[2] * 0.5;
        let click_y = pro_std_btn[1] + pro_std_btn[3] * 0.5;
        rt.pointer_move(click_x, click_y);
        rt.pointer_down(click_x, click_y);
        rt.pointer_up(click_x, click_y);
        rt.frame(1.0 / 60.0);

        assert_eq!(rt.state.recipe.camera_profile, "pro_standard");
        assert!(rt.state.history.can_undo());
        assert_eq!(
            rt.state
                .mcp_shared_canvas
                .lock()
                .unwrap()
                .recipe
                .camera_profile,
            "pro_standard"
        );
        assert_eq!(
            rt.state.active_camera_profile_label(),
            ("Rune ProStandard".to_owned(), false)
        );
    }
}
