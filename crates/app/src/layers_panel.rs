//! Phase 4 Capture One Pro 16.9 Layers, Composable Masks (`Add | Sub | Int`),
//! Local Adjustments, AI Subject/Background & Spot Heal/Clone inspector panel.

use rune::{Cursor, El, Tx, col, el, row, text};
use rune_editor_core::{
    DevelopRecipe, HealMode, MAX_ADJUSTMENT_LAYERS, MAX_HEAL_SPOTS, MAX_MASKS_PER_LAYER,
    MaskCombineOp, MaskShape,
};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit};
use rune_theme_opencode::icons;

use crate::state::{AppState, CanvasTool, InfoTip};
use crate::tokens::{border, color, radius, size, space, ty};
use crate::widgets::{
    advanced_disclosure_row, collapsible_section_card, crop_pill_btn, develop_slider_inner,
    develop_slider_sized, info_badge, mini_reset_btn,
};

/// Compact local-state Scrubbable Capsule for transient brush/heal tool parameters
/// (`brush_radius`, `heal_radius`, etc.) without pushing recipe undo snapshots.
#[allow(clippy::too_many_arguments)]
fn tool_param_capsule(
    k: &Kit,
    key: &'static str,
    label: &'static str,
    value: f32,
    range: std::ops::RangeInclusive<f32>,
    value_fmt: String,
    reset_val: f32,
    setter: fn(&mut AppState, f32),
) -> El {
    let min = *range.start();
    let max = *range.end();
    let span = (max - min).max(1e-6);
    let norm = ((value - min) / span).clamp(0.0, 1.0);
    let width = size::CARD_INNER_W;
    let fill_w = (norm * width).max(0.0);
    let edge_x = (norm * width - size::CAPSULE_EDGE_W * 0.5)
        .clamp(0.0, (width - size::CAPSULE_EDGE_W).max(0.0));

    let content_row = row()
        .w_full()
        .h_full()
        .px(space::XL)
        .items_center()
        .justify_between()
        .child(text(label).font(ty::control_label(k)).color(k.c.text_muted))
        .child(
            text(&value_fmt)
                .font(ty::mono_value(k))
                .color(k.c.text_base),
        );

    let mut capsule = el()
        .key(key)
        .role("slider")
        .label(label)
        .range_value(f64::from(value), f64::from(min), f64::from(max))
        .a11y_value(value_fmt)
        .focusable(true)
        .focus_ring(
            border::FOCUS_RING,
            border::FOCUS_OFFSET_TIGHT,
            k.c.border_focus,
        )
        .cursor(Cursor::ResizeColumn)
        .w(width)
        .h(size::CAPSULE_SLIDER_H)
        .radius(k.radius.lg)
        .fill(color::capsule_bg(k))
        .hover(color::capsule_hover(k))
        .border(border::HAIRLINE, k.c.border_base)
        .clip_rounded(k.radius.lg);

    if fill_w >= 0.75 {
        capsule = capsule
            .child(
                el().absolute(0.0, 0.0)
                    .size(fill_w, size::CAPSULE_SLIDER_H)
                    .fill(color::capsule_fill(k))
                    .a11y_hidden(),
            )
            .child(
                el().absolute(edge_x, 0.0)
                    .size(size::CAPSULE_EDGE_W, size::CAPSULE_SLIDER_H)
                    .fill(color::capsule_edge(k))
                    .a11y_hidden(),
            );
    }

    capsule
        .child(content_row)
        .on_drag(move |s: &mut AppState, d: &mut rune::Drag, _| {
            if d.clicks >= 2 {
                if d.phase == rune::Phase::Down {
                    setter(s, reset_val);
                }
                return;
            }
            let w = d.w.max(1.0);
            let next = (min + (d.x / w).clamp(0.0, 1.0) * span).clamp(min, max);
            setter(s, next);
        })
}

/// Complete Capture One Pro 16.9 `Layers` inspector tab:
/// `LAYERS` stack, `COMBINE MASKS`, `LOCAL ADJUSTMENTS`, and `HEAL & CLONE`.
pub fn layers_tab_content(s: &AppState, k: &Kit) -> El {
    let mut stack = col()
        .w_full()
        .gap(space::CARD_GAP)
        .child(layers_stack_card(s, k));

    if let Some(layer_idx) = s.active_layer_index
        && layer_idx < s.recipe.layers.len()
    {
        stack = stack
            .child(combine_masks_card(s, k, layer_idx))
            .child(local_adjustments_card(s, k, layer_idx));
    }

    stack.child(heal_clone_card(s, k))
}

/// LAYERS Stack Card: quick layer creation, stack rows + Background base row,
/// Opacity, Invert/Clear Mask, and Mask Display (`Off | Red (M) | Gray (⌥M)`).
fn layers_stack_card(s: &AppState, k: &Kit) -> El {
    let layer_count = s.recipe.layers.len();
    let at_cap = layer_count >= MAX_ADJUSTMENT_LAYERS;

    let add_btn = |key: &'static str,
                   label: &'static str,
                   help: &'static str,
                   on_press: fn(&mut AppState, &mut Tx)| {
        row()
            .key(key)
            .role("button")
            .label(help)
            .help(help)
            .focusable(!at_cap)
            .cursor(if at_cap {
                Cursor::Default
            } else {
                Cursor::Pointer
            })
            .grow(1.0)
            .min_w(0.0)
            .h(size::PILL_COMPACT_H)
            .center()
            .radius(k.radius.sm)
            .fill(k.c.bg_base)
            .hover(if at_cap { k.c.bg_base } else { k.c.hover })
            .border(border::HAIRLINE, k.c.border_base)
            .child(
                text(label)
                    .font(ty::control_label(k))
                    .color(if at_cap {
                        k.c.text_faint
                    } else {
                        k.c.text_base
                    })
                    .truncate(),
            )
            .on_press_tx(move |s: &mut AppState, tx: &mut Tx| {
                if s.recipe.layers.len() < MAX_ADJUSTMENT_LAYERS {
                    on_press(s, tx);
                }
            })
    };

    let create_row1 = row()
        .w_full()
        .gap(space::XS)
        .child(add_btn(
            "layer-add-empty",
            "+ Empty",
            "Add empty adjustment layer",
            AppState::add_empty_layer_tx,
        ))
        .child(add_btn(
            "layer-add-filled",
            "+ Filled",
            "Add filled adjustment layer",
            AppState::add_filled_layer_tx,
        ))
        .child(add_btn(
            "layer-add-linear",
            "+ Linear",
            "Add linear gradient mask layer (L)",
            AppState::add_linear_layer_tx,
        ))
        .child(add_btn(
            "layer-add-radial",
            "+ Radial",
            "Add radial gradient mask layer (T)",
            AppState::add_radial_layer_tx,
        ));

    let create_row2 = row()
        .w_full()
        .gap(space::XS)
        .child(add_btn(
            "layer-add-luma",
            "+ Luma",
            "Add luminance range mask layer",
            AppState::add_luma_layer_tx,
        ))
        .child(add_btn(
            "layer-add-ai-subj",
            "+ AI Subject",
            "Auto-detect subject mask",
            |s, tx| s.add_ai_subject_layer_tx(false, tx),
        ))
        .child(add_btn(
            "layer-add-ai-bg",
            "+ AI Bg",
            "Auto-detect background mask",
            |s, tx| s.add_ai_subject_layer_tx(true, tx),
        ));

    // Active canvas mask tools quick strip (`Hand`, `AI Select (W)`, `Brush (B)`, `Erase (E)`, `Linear (L)`, `Radial (T)`)
    let ai_select_tip = InfoTip {
        title: "Interactive AI Select (W)",
        shortcut: Some("W"),
        description: "Click photo to include an object, Option+Click (or − Exclude) to subtract regions, and Shift+Drag to draw a bounding box. Uses SAM 3, SAM 2.1, or Apple Vision ANE.",
        cli_key: "rd layer add --kind ai-select --point x,y,+  ·  MCP: rune_edit_layers",
        tier: "Tier 0 · Smart Selection",
    };

    let tool_strip_header = row()
        .w_full()
        .items_center()
        .justify_between()
        .child(
            row()
                .items_center()
                .gap(space::SM)
                .child(
                    text("MASK TOOLS")
                        .font(ty::section_header(k))
                        .color(k.c.text_faint),
                )
                .child(info_badge(
                    k,
                    ai_select_tip,
                    s.hovered_info
                        .is_some_and(|h| h.title == ai_select_tip.title),
                )),
        )
        .child(
            row()
                .key("layers-ai-models-link")
                .role("button")
                .label("Open AI Models Settings")
                .help("Configure Apple Vision, SAM 2.1, and SAM 3 models in Settings")
                .cursor(Cursor::Pointer)
                .px(space::SM)
                .py(space::MICRO)
                .radius(k.radius.xs)
                .hover(k.c.hover)
                .child(
                    text("AI Models…")
                        .font(ty::mono_micro(k))
                        .color(k.c.text_accent),
                )
                .on_press_tx(|s: &mut AppState, tx| s.open_ai_models_settings_tx(tx)),
        );

    let tool_row1 = row()
        .w_full()
        .gap(space::XS)
        .child(crop_pill_btn(
            k,
            "tool-hand",
            "radio",
            "Hand cursor (Esc)",
            "Hand",
            size::PILL_COMPACT_H,
            s.active_tool == CanvasTool::Hand,
            k.c.text_muted,
            |s, tx| s.set_canvas_tool_tx(CanvasTool::Hand, tx),
        ))
        .child(crop_pill_btn(
            k,
            "tool-ai-select",
            "radio",
            "Interactive AI Select (W)",
            "AI Select (W)",
            size::PILL_COMPACT_H,
            s.active_tool == CanvasTool::AiSelect,
            k.c.text_muted,
            |s, tx| s.set_canvas_tool_tx(CanvasTool::AiSelect, tx),
        ))
        .child(crop_pill_btn(
            k,
            "tool-brush",
            "radio",
            "Brush mask (B)",
            "Brush (B)",
            size::PILL_COMPACT_H,
            s.active_tool == CanvasTool::Brush,
            k.c.text_muted,
            |s, tx| s.set_canvas_tool_tx(CanvasTool::Brush, tx),
        ));

    let tool_row2 = row()
        .w_full()
        .gap(space::XS)
        .child(crop_pill_btn(
            k,
            "tool-eraser",
            "radio",
            "Eraser mask (E)",
            "Erase (E)",
            size::PILL_COMPACT_H,
            s.active_tool == CanvasTool::Eraser,
            k.c.text_muted,
            |s, tx| s.set_canvas_tool_tx(CanvasTool::Eraser, tx),
        ))
        .child(crop_pill_btn(
            k,
            "tool-linear",
            "radio",
            "Linear gradient mask (L)",
            "Linear (L)",
            size::PILL_COMPACT_H,
            s.active_tool == CanvasTool::LinearGradient,
            k.c.text_muted,
            |s, tx| s.set_canvas_tool_tx(CanvasTool::LinearGradient, tx),
        ))
        .child(crop_pill_btn(
            k,
            "tool-radial",
            "radio",
            "Radial gradient mask (T)",
            "Radial (T)",
            size::PILL_COMPACT_H,
            s.active_tool == CanvasTool::RadialGradient,
            k.c.text_muted,
            |s, tx| s.set_canvas_tool_tx(CanvasTool::RadialGradient, tx),
        ));

    let tool_strip = col()
        .key("layer-canvas-tools")
        .role("radiogroup")
        .label("Mask drawing tool")
        .w_full()
        .gap(space::XS)
        .child(tool_strip_header)
        .child(tool_row1)
        .child(tool_row2);

    // Layer stack list (topmost adjustment layer at top, Background at bottom like Capture One).
    let mut stack_list = col().key("layer-stack-list").w_full().gap(space::XS);
    for idx in (0..layer_count).rev() {
        let layer = &s.recipe.layers[idx];
        let selected = s.active_layer_index == Some(idx);
        let badge_label = if layer.masks.len() > 1 {
            format!("Combined ({})", layer.masks.len())
        } else {
            layer.primary_mask_badge().to_owned()
        };
        let vis_dot_color = if layer.enabled {
            k.c.text_accent
        } else {
            k.c.border_strong
        };

        let vis_btn = row()
            .key(format!("layer-vis-{idx}"))
            .role("checkbox")
            .label(format!("Toggle visibility for {}", layer.name))
            .checked(layer.enabled)
            .cursor(Cursor::Pointer)
            .size(16.0, 16.0)
            .center()
            .radius(k.radius.xs)
            .fill(k.c.bg_base)
            .hover(k.c.hover)
            .border(border::HAIRLINE, k.c.border_base)
            .child(
                el().size(6.0, 6.0)
                    .radius(radius::circle(6.0))
                    .fill(vis_dot_color)
                    .a11y_hidden(),
            )
            .on_press_tx(move |s: &mut AppState, tx| s.toggle_layer_enabled_tx(idx, tx));

        let name_btn = row()
            .grow(1.0)
            .min_w(0.0)
            .items_center()
            .gap(space::SM)
            .cursor(Cursor::Pointer)
            .child(
                text(layer.name.as_str())
                    .font(if selected {
                        ty::control_label_medium(k)
                    } else {
                        ty::control_label(k)
                    })
                    .color(if !layer.enabled {
                        k.c.text_faint
                    } else if selected {
                        k.c.text_base
                    } else {
                        k.c.text_muted
                    })
                    .truncate(),
            )
            .child(
                el().px(space::XS)
                    .py(space::MICRO)
                    .radius(k.radius.xs)
                    .fill(k.c.bg_deep)
                    .border(border::HAIRLINE, k.c.border_muted)
                    .child(
                        text(badge_label)
                            .font(ty::mono_micro(k))
                            .color(k.c.text_muted),
                    ),
            )
            .on_press_tx(move |s: &mut AppState, tx| s.select_layer_tx(Some(idx), tx));

        let up_btn = row()
            .key(format!("layer-up-{idx}"))
            .role("button")
            .label("Move layer up")
            .cursor(if idx + 1 < layer_count {
                Cursor::Pointer
            } else {
                Cursor::Default
            })
            .size(16.0, 16.0)
            .center()
            .radius(k.radius.xs)
            .hover(k.c.hover)
            .child(
                text("↑")
                    .font(ty::mono_micro(k))
                    .color(if idx + 1 < layer_count {
                        k.c.text_muted
                    } else {
                        k.c.border_strong
                    }),
            )
            .on_press_tx(move |s: &mut AppState, tx| s.move_layer_tx(idx, 1, tx));

        let dn_btn = row()
            .key(format!("layer-dn-{idx}"))
            .role("button")
            .label("Move layer down")
            .cursor(if idx > 0 {
                Cursor::Pointer
            } else {
                Cursor::Default
            })
            .size(16.0, 16.0)
            .center()
            .radius(k.radius.xs)
            .hover(k.c.hover)
            .child(text("↓").font(ty::mono_micro(k)).color(if idx > 0 {
                k.c.text_muted
            } else {
                k.c.border_strong
            }))
            .on_press_tx(move |s: &mut AppState, tx| s.move_layer_tx(idx, -1, tx));

        let dup_btn = IconButton::new(&icons::COPY)
            .variant(IconButtonVariant::GhostMuted)
            .control_size(ControlSize::Mini)
            .label("Duplicate layer")
            .disabled(at_cap)
            .on_press_tx(move |s: &mut AppState, tx| s.duplicate_layer_tx(idx, tx))
            .build(k);

        let del_btn = IconButton::new(&icons::CLOSE)
            .variant(IconButtonVariant::GhostMuted)
            .control_size(ControlSize::Mini)
            .label("Delete layer")
            .on_press_tx(move |s: &mut AppState, tx| s.remove_layer_tx(idx, tx))
            .build(k);

        let actions = row()
            .items_center()
            .gap(space::MICRO)
            .child(up_btn)
            .child(dn_btn)
            .child(dup_btn)
            .child(del_btn);

        stack_list = stack_list.child(
            row()
                .key(format!("layer-row-{idx}"))
                .w_full()
                .px(space::SM)
                .py(space::XS)
                .items_center()
                .justify_between()
                .gap(space::SM)
                .radius(k.radius.sm)
                .fill(color::pill_fill(k, selected))
                .border(
                    if selected {
                        border::REGULAR
                    } else {
                        border::HAIRLINE
                    },
                    color::pill_border(k, selected),
                )
                .child(vis_btn)
                .child(name_btn)
                .child(actions),
        );
    }

    // Background (Base Image) fixed bottom row
    let bg_selected = s.active_layer_index.is_none();
    stack_list = stack_list.child(
        row()
            .key("layer-row-background")
            .role("button")
            .label("Select Background base image")
            .cursor(Cursor::Pointer)
            .w_full()
            .px(space::SM)
            .py(space::XS)
            .items_center()
            .justify_between()
            .radius(k.radius.sm)
            .fill(color::pill_fill(k, bg_selected))
            .hover(k.c.hover)
            .border(
                if bg_selected {
                    border::REGULAR
                } else {
                    border::HAIRLINE
                },
                color::pill_border(k, bg_selected),
            )
            .child(
                text("Background (Base Image)")
                    .font(if bg_selected {
                        ty::control_label_medium(k)
                    } else {
                        ty::control_label(k)
                    })
                    .color(if bg_selected {
                        k.c.text_base
                    } else {
                        k.c.text_muted
                    }),
            )
            .child(text("Base").font(ty::mono_micro(k)).color(k.c.text_faint))
            .on_press_tx(|s: &mut AppState, tx| s.select_layer_tx(None, tx)),
    );

    let mut body = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(create_row1)
        .child(create_row2)
        .child(tool_strip);

    // Compact interactive AI Select controls sub-panel when AI Select (W) is active
    if s.active_tool == CanvasTool::AiSelect {
        let mode_row = row()
            .key("ai-select-mode-row")
            .role("radiogroup")
            .label("AI Select point polarity")
            .w_full()
            .gap(space::PILL_GAP)
            .child(crop_pill_btn(
                k,
                "ai-sel-include",
                "radio",
                "Positive include points (Click)",
                "+ Include",
                size::PILL_COMPACT_H,
                s.ai_select_include_mode,
                color::ai_point_positive(),
                |s, _| s.ai_select_include_mode = true,
            ))
            .child(crop_pill_btn(
                k,
                "ai-sel-exclude",
                "radio",
                "Negative exclude points (⌥Click)",
                "− Exclude",
                size::PILL_COMPACT_H,
                !s.ai_select_include_mode,
                color::ai_point_negative(),
                |s, _| s.ai_select_include_mode = false,
            ));

        let actions_row = row()
            .key("ai-select-actions-row")
            .w_full()
            .gap(space::XS)
            .child(crop_pill_btn(
                k,
                "ai-sel-invert",
                "button",
                "Invert AI selection mask",
                if s.ai_select_invert {
                    "Inverted ✓"
                } else {
                    "Invert Mask"
                },
                size::PILL_COMPACT_H,
                s.ai_select_invert,
                k.c.text_muted,
                |s, tx| s.ai_select_toggle_invert_tx(tx),
            ))
            .child(crop_pill_btn(
                k,
                "ai-sel-clear",
                "button",
                "Clear AI prompt points and box",
                "Clear Points",
                size::PILL_COMPACT_H,
                false,
                k.c.text_muted,
                |s, tx| s.ai_select_clear_points_tx(tx),
            ))
            .child(crop_pill_btn(
                k,
                "ai-sel-done",
                "button",
                "Finish AI Select session (Esc)",
                "Done",
                size::PILL_COMPACT_H,
                false,
                k.c.text_base,
                |s, tx| s.ai_select_done_tx(tx),
            ));

        let engine_badge_text = s
            .last_ai_mask_status
            .as_ref()
            .map(|st| st.compact_badge())
            .unwrap_or_else(|| s.ai_effective_engine_readout());

        let prompt_count_label = format!(
            "{} pt{}{}",
            s.ai_select_points.len(),
            if s.ai_select_points.len() == 1 {
                ""
            } else {
                "s"
            },
            if s.ai_select_bbox.is_some() {
                " + box"
            } else {
                ""
            }
        );

        let mut ai_box = col()
            .key("ai-select-controls")
            .w_full()
            .gap(space::SM)
            .padding(space::MD)
            .radius(k.radius.md)
            .fill(k.c.bg_deep)
            .border(border::HAIRLINE, k.c.border_base)
            .child(
                row()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap(space::SM)
                    .child(
                        text(engine_badge_text)
                            .font(ty::mono_micro(k))
                            .color(k.c.text_accent)
                            .truncate(),
                    )
                    .child(
                        text(prompt_count_label)
                            .font(ty::mono_micro(k))
                            .color(k.c.text_faint),
                    ),
            )
            .child(mode_row)
            .child(actions_row);

        if let Some(note) = s
            .last_ai_mask_status
            .as_ref()
            .and_then(|st| st.fallback_note.as_deref())
        {
            ai_box = ai_box.child(
                text(format!("Fallback: {note}"))
                    .font(ty::mono_micro(k))
                    .color(color::ai_badge_warning())
                    .wrap(),
            );
        }

        body = body.child(ai_box);
    } else if let Some(ref ai_status) = s.last_ai_mask_status {
        let mut status_col = col()
            .key("ai-mask-status-strip")
            .w_full()
            .gap(space::XXS)
            .px(space::MD)
            .py(space::SM)
            .radius(k.radius.sm)
            .fill(k.c.bg_deep)
            .border(border::HAIRLINE, k.c.border_muted)
            .child(
                row()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap(space::SM)
                    .child(
                        text(ai_status.compact_badge())
                            .font(ty::mono_micro(k))
                            .color(k.c.text_muted)
                            .truncate(),
                    )
                    .child(
                        row()
                            .key("ai-status-models-link")
                            .role("button")
                            .cursor(Cursor::Pointer)
                            .child(
                                text("AI Models…")
                                    .font(ty::mono_micro(k))
                                    .color(k.c.text_accent),
                            )
                            .on_press_tx(|s: &mut AppState, tx| s.open_ai_models_settings_tx(tx)),
                    ),
            );
        if let Some(note) = ai_status.fallback_note.as_deref() {
            status_col = status_col.child(
                text(format!("Fallback: {note}"))
                    .font(ty::mono_micro(k))
                    .color(color::ai_badge_warning())
                    .wrap(),
            );
        }
        body = body.child(status_col);
    }

    body = body.child(stack_list);

    // Active Layer Opacity at Tier 0; Invert/Clear Mask + Mask Display tucked in Tier 1
    if let Some(layer_idx) = s.active_layer_index
        && let Some(layer) = s.recipe.layers.get(layer_idx)
    {
        let opacity_slider = develop_slider_inner(
            k,
            format!("sl-layer-opacity-{layer_idx}"),
            "Opacity",
            None,
            layer.opacity,
            0.0..=1.0,
            format!("{:.0}%", layer.opacity * 100.0),
            1.0,
            true,
            move |rec: &mut DevelopRecipe, v: f32| {
                if let Some(l) = rec.layers.get_mut(layer_idx) {
                    l.opacity = v.clamp(0.0, 1.0);
                }
            },
        );

        let mask_mod = usize::from(layer.inverted) + usize::from(s.camera.mask_overlay_mode != 0);
        body = body.child(opacity_slider).child(advanced_disclosure_row(
            k,
            "adv-layers-mask-actions",
            "Mask Display & Actions",
            s.layers_mask_actions_open,
            mask_mod,
            3,
            |s| s.layers_mask_actions_open = !s.layers_mask_actions_open,
        ));

        if s.layers_mask_actions_open {
            let mask_actions = row()
                .w_full()
                .gap(space::PILL_GAP)
                .child(crop_pill_btn(
                    k,
                    "layer-invert-mask",
                    "button",
                    "Invert active layer mask",
                    if layer.inverted {
                        "Inverted ✓"
                    } else {
                        "Invert Mask"
                    },
                    size::PILL_COMPACT_H,
                    layer.inverted,
                    k.c.text_muted,
                    |s, tx| s.toggle_active_layer_invert_tx(tx),
                ))
                .child(crop_pill_btn(
                    k,
                    "layer-clear-mask",
                    "button",
                    "Clear all masks on active layer",
                    "Clear Mask",
                    size::PILL_COMPACT_H,
                    false,
                    k.c.text_muted,
                    |s, tx| s.clear_active_layer_masks_tx(tx),
                ));

            let mask_display = col()
                .w_full()
                .gap(space::PILL_GAP)
                .child(
                    row()
                        .w_full()
                        .justify_between()
                        .items_center()
                        .child(
                            text("Mask Display")
                                .font(ty::control_label(k))
                                .color(k.c.text_muted),
                        )
                        .child(
                            text(match s.camera.mask_overlay_mode {
                                1 => "Red Rubylith",
                                2 => "Grayscale",
                                _ => "Off",
                            })
                            .font(ty::mono_micro(k))
                            .color(match s.camera.mask_overlay_mode {
                                1 => color::mask_overlay_red(),
                                2 => color::mask_overlay_gray(),
                                _ => k.c.text_faint,
                            }),
                        ),
                )
                .child(
                    row()
                        .key("layer-mask-display")
                        .role("radiogroup")
                        .label("Mask overlay display mode")
                        .w_full()
                        .gap(space::PILL_GAP)
                        .child(crop_pill_btn(
                            k,
                            "mask-disp-off",
                            "radio",
                            "Hide mask overlay",
                            "Off",
                            size::PILL_COMPACT_H,
                            s.camera.mask_overlay_mode == 0,
                            k.c.text_muted,
                            |s, tx| s.set_mask_overlay_mode_tx(0, tx),
                        ))
                        .child(crop_pill_btn(
                            k,
                            "mask-disp-red",
                            "radio",
                            "Red rubylith mask overlay (M)",
                            "Red (M)",
                            size::PILL_COMPACT_H,
                            s.camera.mask_overlay_mode == 1,
                            k.c.text_muted,
                            |s, tx| s.set_mask_overlay_mode_tx(1, tx),
                        ))
                        .child(crop_pill_btn(
                            k,
                            "mask-disp-gray",
                            "radio",
                            "Grayscale mask overlay (⌥M)",
                            "Gray (⌥M)",
                            size::PILL_COMPACT_H,
                            s.camera.mask_overlay_mode == 2,
                            k.c.text_muted,
                            |s, tx| s.set_mask_overlay_mode_tx(2, tx),
                        )),
                );

            body = body.child(mask_actions).child(mask_display);
        }
    }

    let count_badge = text(format!("{layer_count}/{MAX_ADJUSTMENT_LAYERS}"))
        .font(ty::mono_compact(k))
        .color(k.c.text_faint);

    collapsible_section_card(
        k,
        "sec-layers",
        "LAYERS",
        s.layers_open,
        layer_count > 0,
        |s| s.layers_open = !s.layers_open,
        s.hovered_info,
        InfoTip {
            title: "Adjustment Layers & Masks",
            shortcut: Some("W / B / L / T / M"),
            description: "Non-destructive masked adjustment layers supporting Interactive AI Select (W), Linear, Radial, Luma Range, Brush/Eraser, and AI Subject/Background masks.",
            cli_key: "layers[]  ·  MCP: rune_edit_layers",
            tier: "Tier 0 · Layers",
        },
        Some(count_badge),
        body,
    )
}

/// 2. COMBINE MASKS Card for the active layer (`Add | Sub | Int` ops + shape-specific sliders).
fn combine_masks_card(s: &AppState, k: &Kit, layer_idx: usize) -> El {
    let layer = &s.recipe.layers[layer_idx];
    let can_add = layer.masks.len() < MAX_MASKS_PER_LAYER;

    let mut body = col().w_full().gap(space::ROW_GAP);

    if can_add {
        let add_mask_row = row()
            .w_full()
            .gap(space::PILL_GAP)
            .child(crop_pill_btn(
                k,
                "mask-add-linear",
                "button",
                "Add Linear Gradient mask",
                "+ Lin",
                size::PILL_COMPACT_H,
                false,
                k.c.text_muted,
                |s, tx| {
                    s.add_mask_to_active_layer_tx(
                        MaskShape::LinearGradient {
                            start: [0.5, 0.15],
                            end: [0.5, 0.65],
                            feather: 0.75,
                        },
                        tx,
                    );
                },
            ))
            .child(crop_pill_btn(
                k,
                "mask-add-radial",
                "button",
                "Add Radial Gradient mask",
                "+ Rad",
                size::PILL_COMPACT_H,
                false,
                k.c.text_muted,
                |s, tx| {
                    s.add_mask_to_active_layer_tx(
                        MaskShape::RadialGradient {
                            center: [0.5, 0.46],
                            radius: [0.26, 0.32],
                            angle_deg: 0.0,
                            feather: 0.45,
                        },
                        tx,
                    );
                },
            ))
            .child(crop_pill_btn(
                k,
                "mask-add-luma",
                "button",
                "Add Luma Range mask",
                "+ Luma",
                size::PILL_COMPACT_H,
                false,
                k.c.text_muted,
                |s, tx| {
                    s.add_mask_to_active_layer_tx(
                        MaskShape::LumaRange {
                            low: 0.45,
                            high: 1.0,
                            low_falloff: 0.15,
                            high_falloff: 0.05,
                        },
                        tx,
                    );
                },
            ))
            .child(crop_pill_btn(
                k,
                "mask-add-filled",
                "button",
                "Add Filled mask",
                "+ Fill",
                size::PILL_COMPACT_H,
                false,
                k.c.text_muted,
                |s, tx| s.add_mask_to_active_layer_tx(MaskShape::Filled, tx),
            ));
        body = body.child(add_mask_row);
    }

    if layer.masks.is_empty() {
        body = body.child(
            text("No masks on this layer yet.")
                .font(ty::caption(k))
                .color(k.c.text_faint),
        );
    }

    for (mask_idx, item) in layer.masks.iter().enumerate() {
        let mut op_row = row().items_center().gap(space::XXS);
        for op in MaskCombineOp::ALL {
            let active = item.op == op;
            op_row = op_row.child(
                row()
                    .key(format!("mask-op-{layer_idx}-{mask_idx}-{}", op.cli_slug()))
                    .role("radio")
                    .label(format!("Combine mode {}", op.label()))
                    .checked(active)
                    .cursor(Cursor::Pointer)
                    .h(22.0)
                    .px(space::SM)
                    .center()
                    .radius(k.radius.xs)
                    .fill(color::pill_fill(k, active))
                    .hover(k.c.hover)
                    .border(border::HAIRLINE, color::pill_border(k, active))
                    .child(text(op.label()).font(ty::mono_micro(k)).color(if active {
                        k.c.text_accent
                    } else {
                        k.c.text_muted
                    }))
                    .on_press_tx(move |s: &mut AppState, tx| {
                        s.set_layer_mask_op_tx(layer_idx, mask_idx, op, tx);
                    }),
            );
        }

        let inv_pill = row()
            .key(format!("mask-inv-{layer_idx}-{mask_idx}"))
            .role("button")
            .label("Invert this mask item")
            .cursor(Cursor::Pointer)
            .h(22.0)
            .px(space::SM)
            .center()
            .radius(k.radius.xs)
            .fill(color::pill_fill(k, item.inverted))
            .hover(k.c.hover)
            .border(border::HAIRLINE, color::pill_border(k, item.inverted))
            .child(text("Inv").font(ty::mono_micro(k)).color(if item.inverted {
                k.c.text_accent
            } else {
                k.c.text_muted
            }))
            .on_press_tx(move |s: &mut AppState, tx| {
                s.toggle_layer_mask_invert_tx(layer_idx, mask_idx, tx);
            });

        let rm_btn = IconButton::new(&icons::CLOSE)
            .variant(IconButtonVariant::GhostMuted)
            .control_size(ControlSize::Mini)
            .label("Remove mask")
            .on_press_tx(move |s: &mut AppState, tx| {
                s.remove_layer_mask_tx(layer_idx, mask_idx, tx);
            })
            .build(k);

        let header = row()
            .w_full()
            .items_center()
            .justify_between()
            .gap(space::XS)
            .child(
                text(format!("{}. {}", mask_idx + 1, item.shape.kind_label()))
                    .font(ty::control_label_medium(k))
                    .color(k.c.text_base),
            )
            .child(
                row()
                    .items_center()
                    .gap(space::XS)
                    .child(op_row)
                    .child(inv_pill)
                    .child(rm_btn),
            );

        let mut item_box = col()
            .key(format!("mask-item-{layer_idx}-{mask_idx}"))
            .w_full()
            .gap(space::XS)
            .padding(space::SM)
            .radius(k.radius.sm)
            .fill(k.c.bg_deep)
            .border(border::HAIRLINE, k.c.border_muted)
            .child(header);

        let nested_w = size::CARD_INNER_W - space::SM * 2.0 - 2.0;
        match &item.shape {
            MaskShape::LinearGradient { feather, .. } => {
                item_box = item_box.child(develop_slider_sized(
                    k,
                    format!("sl-lin-feather-{layer_idx}-{mask_idx}"),
                    "Feather",
                    None,
                    nested_w,
                    *feather,
                    0.0..=1.0,
                    format!("{:.0}%", feather * 100.0),
                    0.75,
                    true,
                    move |rec: &mut DevelopRecipe, v: f32| {
                        if let Some(l) = rec.layers.get_mut(layer_idx)
                            && let Some(m) = l.masks.get_mut(mask_idx)
                            && let MaskShape::LinearGradient { feather, .. } = &mut m.shape
                        {
                            *feather = v.clamp(0.0, 1.0);
                        }
                    },
                ));
            }
            MaskShape::RadialGradient {
                feather, angle_deg, ..
            } => {
                item_box = item_box
                    .child(develop_slider_sized(
                        k,
                        format!("sl-rad-feather-{layer_idx}-{mask_idx}"),
                        "Feather",
                        None,
                        nested_w,
                        *feather,
                        0.0..=1.0,
                        format!("{:.0}%", feather * 100.0),
                        0.45,
                        true,
                        move |rec: &mut DevelopRecipe, v: f32| {
                            if let Some(l) = rec.layers.get_mut(layer_idx)
                                && let Some(m) = l.masks.get_mut(mask_idx)
                                && let MaskShape::RadialGradient { feather, .. } = &mut m.shape
                            {
                                *feather = v.clamp(0.0, 1.0);
                            }
                        },
                    ))
                    .child(develop_slider_sized(
                        k,
                        format!("sl-rad-angle-{layer_idx}-{mask_idx}"),
                        "Angle",
                        None,
                        nested_w,
                        *angle_deg,
                        -180.0..=180.0,
                        format!("{:+.0}°", angle_deg),
                        0.0,
                        true,
                        move |rec: &mut DevelopRecipe, v: f32| {
                            if let Some(l) = rec.layers.get_mut(layer_idx)
                                && let Some(m) = l.masks.get_mut(mask_idx)
                                && let MaskShape::RadialGradient { angle_deg, .. } = &mut m.shape
                            {
                                *angle_deg = v.clamp(-180.0, 180.0);
                            }
                        },
                    ));
            }
            MaskShape::LumaRange {
                low,
                high,
                low_falloff,
                high_falloff,
            } => {
                item_box = item_box
                    .child(develop_slider_sized(
                        k,
                        format!("sl-luma-low-{layer_idx}-{mask_idx}"),
                        "Luma Low",
                        None,
                        nested_w,
                        *low,
                        0.0..=1.0,
                        format!("{:.0}%", low * 100.0),
                        0.0,
                        true,
                        move |rec: &mut DevelopRecipe, v: f32| {
                            if let Some(l) = rec.layers.get_mut(layer_idx)
                                && let Some(m) = l.masks.get_mut(mask_idx)
                                && let MaskShape::LumaRange { low, high, .. } = &mut m.shape
                            {
                                *low = v.clamp(0.0, *high);
                            }
                        },
                    ))
                    .child(develop_slider_sized(
                        k,
                        format!("sl-luma-high-{layer_idx}-{mask_idx}"),
                        "Luma High",
                        None,
                        nested_w,
                        *high,
                        0.0..=1.0,
                        format!("{:.0}%", high * 100.0),
                        1.0,
                        true,
                        move |rec: &mut DevelopRecipe, v: f32| {
                            if let Some(l) = rec.layers.get_mut(layer_idx)
                                && let Some(m) = l.masks.get_mut(mask_idx)
                                && let MaskShape::LumaRange { low, high, .. } = &mut m.shape
                            {
                                *high = v.clamp(*low, 1.0);
                            }
                        },
                    ))
                    .child(develop_slider_sized(
                        k,
                        format!("sl-luma-lfall-{layer_idx}-{mask_idx}"),
                        "Low Falloff",
                        None,
                        nested_w,
                        *low_falloff,
                        0.0..=0.5,
                        format!("{:.0}%", low_falloff * 100.0),
                        0.15,
                        true,
                        move |rec: &mut DevelopRecipe, v: f32| {
                            if let Some(l) = rec.layers.get_mut(layer_idx)
                                && let Some(m) = l.masks.get_mut(mask_idx)
                                && let MaskShape::LumaRange { low_falloff, .. } = &mut m.shape
                            {
                                *low_falloff = v.clamp(0.0, 0.5);
                            }
                        },
                    ))
                    .child(develop_slider_sized(
                        k,
                        format!("sl-luma-hfall-{layer_idx}-{mask_idx}"),
                        "High Falloff",
                        None,
                        nested_w,
                        *high_falloff,
                        0.0..=0.5,
                        format!("{:.0}%", high_falloff * 100.0),
                        0.05,
                        true,
                        move |rec: &mut DevelopRecipe, v: f32| {
                            if let Some(l) = rec.layers.get_mut(layer_idx)
                                && let Some(m) = l.masks.get_mut(mask_idx)
                                && let MaskShape::LumaRange { high_falloff, .. } = &mut m.shape
                            {
                                *high_falloff = v.clamp(0.0, 0.5);
                            }
                        },
                    ));
            }
            MaskShape::BrushStrokes { strokes } => {
                item_box = item_box.child(
                    text(format!(
                        "{} brush dab(s) — use Brush (B) or Eraser (E) on canvas",
                        strokes.len()
                    ))
                    .font(ty::caption(k))
                    .color(k.c.text_muted),
                );
            }
            MaskShape::RasterMask { width, height, .. } => {
                let desc = if let Some(ref st) = s.last_ai_mask_status {
                    format!("{width}×{height} AI Smart Mask · {}", st.compact_badge())
                } else {
                    format!("{width}×{height} AI Smart Mask")
                };
                item_box = item_box.child(text(desc).font(ty::caption(k)).color(k.c.text_muted));
            }
            MaskShape::Filled => {}
        }

        body = body.child(item_box);
    }

    // Brush size / hardness / opacity controls when Brush or Eraser tool is active
    if matches!(s.active_tool, CanvasTool::Brush | CanvasTool::Eraser) {
        body = body
            .child(tool_param_capsule(
                k,
                "sl-brush-radius",
                "Brush Size",
                s.brush_radius,
                0.01..=0.25,
                format!("{:.0}%", s.brush_radius * 100.0),
                0.06,
                |s, v| s.brush_radius = v,
            ))
            .child(tool_param_capsule(
                k,
                "sl-brush-hardness",
                "Hardness",
                s.brush_hardness,
                0.0..=1.0,
                format!("{:.0}%", s.brush_hardness * 100.0),
                0.45,
                |s, v| s.brush_hardness = v,
            ))
            .child(tool_param_capsule(
                k,
                "sl-brush-opacity",
                "Flow / Opacity",
                s.brush_opacity,
                0.05..=1.0,
                format!("{:.0}%", s.brush_opacity * 100.0),
                0.85,
                |s, v| s.brush_opacity = v,
            ));
    }

    let badge = text(format!("{}/{MAX_MASKS_PER_LAYER}", layer.masks.len()))
        .font(ty::mono_compact(k))
        .color(k.c.text_faint);

    collapsible_section_card(
        k,
        "sec-combine-masks",
        "COMBINE MASKS",
        s.masks_open,
        !layer.masks.is_empty(),
        |s| s.masks_open = !s.masks_open,
        s.hovered_info,
        InfoTip {
            title: "Composable Boolean Masks",
            shortcut: Some("M / ⌥M"),
            description: "Combines up to 4 mask shapes per layer using Add, Subtract, and Intersect operators with per-mask inversion and feathering.",
            cli_key: "layers[].masks[]  ·  MCP: rune_edit_layers",
            tier: "Tier 1 · Masking",
        },
        Some(badge),
        body,
    )
}

/// 3. LOCAL ADJUSTMENTS Card for the active layer (Tier 0 primary 5 + Tier 1 Color & Detail 6).
fn local_adjustments_card(s: &AppState, k: &Kit, layer_idx: usize) -> El {
    let adj = s.recipe.layers[layer_idx].adjustments;
    let neutral = adj.is_neutral();
    let adv_mod = usize::from(adj.brightness.abs() > 0.005)
        + usize::from(adj.tint.abs() > 0.005)
        + usize::from(adj.saturation.abs() > 0.005)
        + usize::from(adj.clarity.abs() > 0.005)
        + usize::from(adj.dehaze.abs() > 0.005)
        + usize::from(adj.sharpen.abs() > 0.005);

    let mut controls = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(develop_slider_inner(
            k,
            format!("sl-loc-exp-{layer_idx}"),
            "Exposure",
            None,
            adj.exposure,
            -4.0..=4.0,
            format!("{:+.2} EV", adj.exposure),
            0.0,
            true,
            move |rec, v| {
                if let Some(l) = rec.layers.get_mut(layer_idx) {
                    l.adjustments.exposure = v.clamp(-4.0, 4.0);
                }
            },
        ))
        .child(develop_slider_inner(
            k,
            format!("sl-loc-con-{layer_idx}"),
            "Contrast",
            None,
            adj.contrast,
            -1.0..=1.0,
            format!("{:+.0}", adj.contrast * 100.0),
            0.0,
            true,
            move |rec, v| {
                if let Some(l) = rec.layers.get_mut(layer_idx) {
                    l.adjustments.contrast = v.clamp(-1.0, 1.0);
                }
            },
        ))
        .child(develop_slider_inner(
            k,
            format!("sl-loc-hi-{layer_idx}"),
            "Highlights",
            None,
            adj.highlights,
            -1.0..=1.0,
            format!("{:+.0}", adj.highlights * 100.0),
            0.0,
            true,
            move |rec, v| {
                if let Some(l) = rec.layers.get_mut(layer_idx) {
                    l.adjustments.highlights = v.clamp(-1.0, 1.0);
                }
            },
        ))
        .child(develop_slider_inner(
            k,
            format!("sl-loc-sh-{layer_idx}"),
            "Shadows",
            None,
            adj.shadows,
            -1.0..=1.0,
            format!("{:+.0}", adj.shadows * 100.0),
            0.0,
            true,
            move |rec, v| {
                if let Some(l) = rec.layers.get_mut(layer_idx) {
                    l.adjustments.shadows = v.clamp(-1.0, 1.0);
                }
            },
        ))
        .child(develop_slider_inner(
            k,
            format!("sl-loc-temp-{layer_idx}"),
            "Temp",
            None,
            adj.temperature,
            -1.0..=1.0,
            format!("{:+.0}", adj.temperature * 100.0),
            0.0,
            true,
            move |rec, v| {
                if let Some(l) = rec.layers.get_mut(layer_idx) {
                    l.adjustments.temperature = v.clamp(-1.0, 1.0);
                }
            },
        ))
        .child(advanced_disclosure_row(
            k,
            "adv-local-adj-more",
            "Color & Detail",
            s.local_adj_advanced_open,
            adv_mod,
            6,
            |s| s.local_adj_advanced_open = !s.local_adj_advanced_open,
        ));

    if s.local_adj_advanced_open {
        controls = controls
            .child(develop_slider_inner(
                k,
                format!("sl-loc-bri-{layer_idx}"),
                "Brightness",
                None,
                adj.brightness,
                -1.0..=1.0,
                format!("{:+.0}", adj.brightness * 100.0),
                0.0,
                true,
                move |rec, v| {
                    if let Some(l) = rec.layers.get_mut(layer_idx) {
                        l.adjustments.brightness = v.clamp(-1.0, 1.0);
                    }
                },
            ))
            .child(develop_slider_inner(
                k,
                format!("sl-loc-tint-{layer_idx}"),
                "Tint",
                None,
                adj.tint,
                -1.0..=1.0,
                format!("{:+.0}", adj.tint * 100.0),
                0.0,
                true,
                move |rec, v| {
                    if let Some(l) = rec.layers.get_mut(layer_idx) {
                        l.adjustments.tint = v.clamp(-1.0, 1.0);
                    }
                },
            ))
            .child(develop_slider_inner(
                k,
                format!("sl-loc-sat-{layer_idx}"),
                "Saturation",
                None,
                adj.saturation,
                -1.0..=1.0,
                format!("{:+.0}", adj.saturation * 100.0),
                0.0,
                true,
                move |rec, v| {
                    if let Some(l) = rec.layers.get_mut(layer_idx) {
                        l.adjustments.saturation = v.clamp(-1.0, 1.0);
                    }
                },
            ))
            .child(develop_slider_inner(
                k,
                format!("sl-loc-clr-{layer_idx}"),
                "Clarity",
                None,
                adj.clarity,
                -1.0..=1.0,
                format!("{:+.0}", adj.clarity * 100.0),
                0.0,
                true,
                move |rec, v| {
                    if let Some(l) = rec.layers.get_mut(layer_idx) {
                        l.adjustments.clarity = v.clamp(-1.0, 1.0);
                    }
                },
            ))
            .child(develop_slider_inner(
                k,
                format!("sl-loc-dhz-{layer_idx}"),
                "Dehaze",
                None,
                adj.dehaze,
                -1.0..=1.0,
                format!("{:+.0}", adj.dehaze * 100.0),
                0.0,
                true,
                move |rec, v| {
                    if let Some(l) = rec.layers.get_mut(layer_idx) {
                        l.adjustments.dehaze = v.clamp(-1.0, 1.0);
                    }
                },
            ))
            .child(develop_slider_inner(
                k,
                format!("sl-loc-shp-{layer_idx}"),
                "Sharpen",
                None,
                adj.sharpen,
                0.0..=1.0,
                format!("{:.0}", adj.sharpen * 100.0),
                0.0,
                true,
                move |rec, v| {
                    if let Some(l) = rec.layers.get_mut(layer_idx) {
                        l.adjustments.sharpen = v.clamp(0.0, 1.0);
                    }
                },
            ));
    }

    let reset = mini_reset_btn(k, "Reset Local Adjustments", neutral, |s, tx| {
        s.reset_active_layer_adjustments_tx(tx);
    });

    collapsible_section_card(
        k,
        "sec-local-adj",
        "LOCAL ADJUSTMENTS",
        s.local_adj_open,
        !neutral,
        |s| s.local_adj_open = !s.local_adj_open,
        s.hovered_info,
        InfoTip {
            title: "Per-Layer Local Adjustments",
            shortcut: None,
            description: "Blends Exposure, Contrast, Highlights, Shadows, Temperature, and Color/Detail selectively through the active layer's combined mask.",
            cli_key: "layers[].adjustments.*  ·  MCP: rune_edit_layers",
            tier: "Tier 0 · Local Grade",
        },
        Some(reset),
        controls,
    )
}

/// HEAL & CLONE Card: mode selector (`Heal (Q)` | `Clone (S)`), Radius/Feather at Tier 0,
/// Opacity + Clear Spots in Tier 1, and active spot list.
fn heal_clone_card(s: &AppState, k: &Kit) -> El {
    let spot_count = s.recipe.heal_spots.len();
    let heal_active = s.active_tool == CanvasTool::Heal;
    let clone_active = s.active_tool == CanvasTool::Clone;

    let mode_row = row()
        .key("heal-mode-row")
        .role("radiogroup")
        .label("Heal and Clone tool mode")
        .w_full()
        .gap(space::PILL_GAP)
        .child(crop_pill_btn(
            k,
            "tool-heal-mode",
            "radio",
            "Spot Heal tool (Q)",
            "Heal (Q)",
            size::PILL_H,
            heal_active,
            k.c.text_muted,
            |s, tx| {
                let next = if s.active_tool == CanvasTool::Heal {
                    CanvasTool::Hand
                } else {
                    CanvasTool::Heal
                };
                s.set_canvas_tool_tx(next, tx);
            },
        ))
        .child(crop_pill_btn(
            k,
            "tool-clone-mode",
            "radio",
            "Clone Stamp tool (S)",
            "Clone (S)",
            size::PILL_H,
            clone_active,
            k.c.text_muted,
            |s, tx| {
                let next = if s.active_tool == CanvasTool::Clone {
                    CanvasTool::Hand
                } else {
                    CanvasTool::Clone
                };
                s.set_canvas_tool_tx(next, tx);
            },
        ));

    let heal_adv_mod = usize::from((s.heal_opacity - 1.0).abs() > 0.01);
    let mut body = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(mode_row)
        .child(tool_param_capsule(
            k,
            "sl-heal-radius",
            "Radius",
            s.heal_radius,
            0.005..=0.15,
            format!("{:.1}%", s.heal_radius * 100.0),
            0.032,
            |s, v| s.heal_radius = v,
        ))
        .child(tool_param_capsule(
            k,
            "sl-heal-feather",
            "Feather",
            s.heal_feather,
            0.0..=1.0,
            format!("{:.0}%", s.heal_feather * 100.0),
            0.42,
            |s, v| s.heal_feather = v,
        ));

    if !s.recipe.heal_spots.is_empty() {
        let mut list = col().w_full().gap(space::XS);
        for (idx, spot) in s.recipe.heal_spots.iter().enumerate() {
            let dot_col = match spot.mode {
                HealMode::Heal => color::heal_target_ring(),
                HealMode::Clone => color::clone_target_ring(),
            };
            list = list.child(
                row()
                    .key(format!("heal-spot-row-{idx}"))
                    .w_full()
                    .px(space::SM)
                    .py(space::XS)
                    .items_center()
                    .justify_between()
                    .radius(k.radius.sm)
                    .fill(k.c.bg_deep)
                    .border(border::HAIRLINE, k.c.border_muted)
                    .child(
                        row()
                            .items_center()
                            .gap(space::SM)
                            .child(
                                el().size(size::CHANNEL_DOT, size::CHANNEL_DOT)
                                    .radius(radius::circle(size::CHANNEL_DOT))
                                    .fill(dot_col)
                                    .a11y_hidden(),
                            )
                            .child(
                                text(format!(
                                    "#{} {} ({:.0}%, {:.0}%)",
                                    idx + 1,
                                    spot.mode.label(),
                                    spot.target[0] * 100.0,
                                    spot.target[1] * 100.0
                                ))
                                .font(ty::mono_micro(k))
                                .color(k.c.text_muted),
                            ),
                    )
                    .child(
                        IconButton::new(&icons::CLOSE)
                            .variant(IconButtonVariant::GhostMuted)
                            .control_size(ControlSize::Mini)
                            .label(format!("Remove spot #{}", idx + 1))
                            .on_press_tx(move |s: &mut AppState, tx| {
                                s.remove_heal_spot_tx(idx, tx);
                            })
                            .build(k),
                    ),
            );
        }
        body = body.child(list);
    }

    body = body.child(advanced_disclosure_row(
        k,
        "adv-heal-edge",
        "Opacity & Actions",
        s.heal_advanced_open,
        heal_adv_mod,
        2,
        |s| s.heal_advanced_open = !s.heal_advanced_open,
    ));

    if s.heal_advanced_open {
        body = body
            .child(tool_param_capsule(
                k,
                "sl-heal-opacity",
                "Opacity",
                s.heal_opacity,
                0.05..=1.0,
                format!("{:.0}%", s.heal_opacity * 100.0),
                1.0,
                |s, v| s.heal_opacity = v,
            ))
            .child(
                row().w_full().justify_end().child(
                    Button::new("Clear Spots")
                        .variant(ButtonVariant::GhostMuted)
                        .control_size(ControlSize::Small)
                        .disabled(spot_count == 0)
                        .on_press_tx(|s: &mut AppState, tx| s.clear_heal_spots_tx(tx))
                        .build(k),
                ),
            );
    }

    let badge = text(format!("{spot_count}/{MAX_HEAL_SPOTS}"))
        .font(ty::mono_compact(k))
        .color(k.c.text_faint);

    collapsible_section_card(
        k,
        "sec-heal-clone",
        "HEAL & CLONE",
        s.heal_open,
        spot_count > 0,
        |s| s.heal_open = !s.heal_open,
        s.hovered_info,
        InfoTip {
            title: "Spot Heal & Clone Stamp",
            shortcut: Some("Q / S"),
            description: "Non-destructive blemish removal and patch cloning. Click photo to place a spot, drag to aim the source patch, press Esc for Hand.",
            cli_key: "heal_spots[]  ·  MCP: rune_edit_layers",
            tier: "Tier 1 · Retouch",
        },
        Some(badge),
        body,
    )
}
