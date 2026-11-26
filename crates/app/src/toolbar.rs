//! Unified Apple macOS Tahoe HIG titlebar — Concept A Minimal Pro + `Overlays ▾` Popover.
//!
//! Respects `Cx::chrome_insets()` so macOS traffic lights (`[16, 17, 14, 14]`) never collide
//! with titlebar controls, enables native `.window_drag()` on empty bar area, and keeps
//! diagnostic overlays tucked inside a crisp frosted `Overlays ▾` popover.

use rune::{ChromeInsets, Color, Cursor, El, Tx, col, el, icon, row, text};
use rune_editor_core::CompositionGrid;
use rune_kit::{
    Badge, BadgeVariant, Button, ButtonVariant, ControlSize, Divider, Kit, SegmentedControl,
    scrim_tx,
};
use rune_theme_opencode::icons;

use crate::state::AppState;
use crate::tokens::{border, color, radius, size, space, ty};

pub fn top_toolbar(s: &AppState, k: &Kit, insets: ChromeInsets) -> El {
    let left_pad = insets.leading + space::XXL;
    let right_pad = insets.trailing + space::XXL;
    let band_h = insets.height.max(size::TITLEBAR_H);

    // Left cluster: Traffic lights safe inset + brand + document filename + Edited badge
    let mut left = row()
        .key("toolbar-left")
        .items_center()
        .gap(space::MD)
        .min_w(0.0)
        .shrink()
        .no_window_drag()
        .child(
            row()
                .items_center()
                .gap(space::SM)
                .child(
                    icon(&icons::PHOTO, size::ICON_LG)
                        .color(k.c.icon_base)
                        .a11y_hidden(),
                )
                .child(
                    text("Rune Darkroom")
                        .font(ty::brand_title(k))
                        .color(k.c.text_base),
                ),
        )
        .child(el_v_divider(k))
        .child(
            text(s.image.display_name.as_str())
                .font(ty::control_label(k))
                .color(k.c.text_muted)
                .truncate(),
        );

    if s.is_dirty() {
        left = left.child(Badge::new("Edited").variant(BadgeVariant::Accent).build(k));
    }

    // Center cluster: Fit | 150% | 200% zoom control + Compare button
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
        .gap(space::PILL_GAP)
        .no_window_drag()
        .child(zoom_seg)
        .child(split_btn);

    // Right cluster: Overlays ▾ popover trigger, Grid, Settings (with live MCP green dot), Export
    let active_overlays = s.active_overlay_count();
    let overlays_open = s.overlays_popover_open;
    let mut overlays_btn = row()
        .key("btn-overlays-popover")
        .role("button")
        .label("Overlays and diagnostic toggles")
        .help("Toggle Exposure Clipping, Focus Mask, Mask Overlay, Loupe & Composition Grid")
        .expanded(overlays_open)
        .focusable(true)
        .focus_ring(
            border::FOCUS_RING,
            border::FOCUS_OFFSET_TIGHT,
            k.c.border_focus,
        )
        .cursor(Cursor::Pointer)
        .h(size::ACTION_PILL_H)
        .px(space::LG)
        .items_center()
        .gap(space::SM)
        .radius(k.radius.md)
        .continuous_corners()
        .fill(if overlays_open {
            k.c.layer_03
        } else if active_overlays > 0 {
            k.c.layer_02
        } else {
            k.c.layer_01
        })
        .hover(k.c.layer_02)
        .border(
            border::HAIRLINE,
            if overlays_open {
                k.c.border_focus
            } else if active_overlays > 0 {
                color::mask_guide_primary()
            } else {
                k.c.border_base
            },
        )
        .child(
            icon(&icons::EYE, size::ICON_MD)
                .color(if overlays_open || active_overlays > 0 {
                    k.c.text_base
                } else {
                    k.c.icon_muted
                })
                .a11y_hidden(),
        )
        .child(text("Overlays").font(ty::control_label_medium(k)).color(
            if overlays_open || active_overlays > 0 {
                k.c.text_base
            } else {
                k.c.text_muted
            },
        ));

    if active_overlays > 0 {
        overlays_btn = overlays_btn.child(
            row()
                .px(space::XS)
                .h(16.0)
                .center()
                .radius(radius::circle(16.0))
                .fill(color::mask_guide_primary().alpha(0.22))
                .child(
                    text(format!("{active_overlays}"))
                        .font(ty::mono_probe(k))
                        .color(color::mask_guide_primary()),
                ),
        );
    }

    let overlays_btn = overlays_btn
        .child(
            icon(&icons::CHEVRON_DOWN, size::ICON_XS)
                .color(if overlays_open {
                    k.c.text_base
                } else {
                    k.c.icon_muted
                })
                .a11y_hidden(),
        )
        .on_press(|s: &mut AppState| s.toggle_overlays_popover());

    let grid_btn = Button::new("Grid")
        .variant(if s.show_grid_browser {
            ButtonVariant::Contrast
        } else {
            ButtonVariant::Neutral
        })
        .control_size(ControlSize::Regular)
        .icon(&icons::GRID_PLUS)
        .help("Toggle multi-column Grid Browser view")
        .shortcut("G")
        .on_press_tx(|s: &mut AppState, tx| s.toggle_grid_browser_tx(tx))
        .build(k);

    let mcp_running = s.is_mcp_running();
    let mut settings_btn = row()
        .key("btn-toolbar-settings")
        .role("button")
        .label("Settings and Local MCP")
        .help("Open Settings & Local MCP Server controls (⌘,)")
        .focusable(true)
        .focus_ring(
            border::FOCUS_RING,
            border::FOCUS_OFFSET_TIGHT,
            k.c.border_focus,
        )
        .cursor(Cursor::Pointer)
        .h(size::ACTION_PILL_H)
        .px(space::LG)
        .items_center()
        .gap(space::SM)
        .radius(k.radius.md)
        .continuous_corners()
        .fill(if s.show_settings {
            k.c.layer_03
        } else {
            k.c.layer_01
        })
        .hover(k.c.layer_02)
        .border(
            border::HAIRLINE,
            if s.show_settings {
                k.c.border_focus
            } else {
                k.c.border_base
            },
        )
        .child(
            icon(&icons::MCP, size::ICON_MD)
                .color(if s.show_settings || mcp_running {
                    k.c.text_base
                } else {
                    k.c.icon_muted
                })
                .a11y_hidden(),
        )
        .child(
            text("Settings")
                .font(ty::control_label_medium(k))
                .color(if s.show_settings {
                    k.c.text_base
                } else {
                    k.c.text_muted
                }),
        );

    if mcp_running {
        settings_btn = settings_btn.child(
            el().size(size::STATUS_DOT, size::STATUS_DOT)
                .radius(radius::circle(size::STATUS_DOT))
                .fill(color::mcp_running())
                .a11y_hidden(),
        );
    }

    let settings_btn = settings_btn.on_press_tx(|s: &mut AppState, tx| {
        let next = !s.show_settings;
        s.set_show_settings_tx(next, tx);
    });

    let export_btn = Button::new("Export")
        .variant(ButtonVariant::Contrast)
        .control_size(ControlSize::Regular)
        .icon(&icons::SHARE)
        .help("Open Multi-Recipe Export view (⌘⇧E)")
        .shortcut("cmd+shift+e")
        .on_press_tx(|s: &mut AppState, tx| {
            let next = !s.show_export;
            s.set_show_export_tx(next, tx);
        })
        .build(k);

    let right = row()
        .key("toolbar-right")
        .items_center()
        .gap(space::PILL_GAP)
        .no_window_drag()
        .child(overlays_btn)
        .child(grid_btn)
        .child(settings_btn)
        .child(export_btn);

    row()
        .key("toolbar")
        .role("toolbar")
        .label("Darkroom toolbar")
        .w_full()
        .h(band_h)
        .pl(left_pad)
        .pr(right_pad)
        .gap(space::LG)
        .center_y()
        .justify_between()
        .fill(k.c.bg_deep)
        .border_sides(
            [border::NONE, border::NONE, border::REGULAR, border::NONE],
            k.c.border_base,
        )
        .window_drag()
        .child(left)
        .child(center)
        .child(right)
}

/// Crisp floating macOS popover anchored cleanly below the Concept A titlebar `Overlays ▾` button.
pub fn overlays_popover_overlay(s: &AppState, k: &Kit) -> El {
    let toggle_row = |key: &'static str,
                      label: &'static str,
                      shortcut: &'static str,
                      active: bool,
                      accent: Color,
                      on_press: fn(&mut AppState, &mut Tx)| {
        row()
            .key(key)
            .role("menuitemcheckbox")
            .label(label)
            .checked(active)
            .cursor(Cursor::Pointer)
            .w_full()
            .h(size::ACTION_PILL_H)
            .px(space::MD)
            .items_center()
            .justify_between()
            .radius(k.radius.md)
            .fill(if active {
                k.c.layer_02
            } else {
                Color::TRANSPARENT
            })
            .hover(k.c.layer_02)
            .child(
                row()
                    .items_center()
                    .gap(space::MD)
                    .child(
                        el().size(size::STATUS_DOT, size::STATUS_DOT)
                            .radius(radius::circle(size::STATUS_DOT))
                            .fill(if active { accent } else { k.c.border_strong })
                            .a11y_hidden(),
                    )
                    .child(
                        text(label)
                            .font(if active {
                                ty::control_label_medium(k)
                            } else {
                                ty::control_label(k)
                            })
                            .color(if active {
                                k.c.text_base
                            } else {
                                k.c.text_muted
                            }),
                    ),
            )
            .child(
                el().px(space::SM)
                    .py(space::MICRO)
                    .radius(k.radius.xs)
                    .fill(k.c.bg_deep)
                    .border(border::HAIRLINE, k.c.border_muted)
                    .child(text(shortcut).font(ty::mono_probe(k)).color(if active {
                        accent
                    } else {
                        k.c.text_faint
                    })),
            )
            .on_press_tx(on_press)
    };

    let grid_label = match s.recipe.geometry.grid {
        CompositionGrid::None => "Off",
        CompositionGrid::RuleOfThirds => "Thirds",
        CompositionGrid::GoldenRatio => "Golden",
        CompositionGrid::Diagonal => "Diag",
    };
    let grid_active = s.recipe.geometry.grid != CompositionGrid::None;

    let card = k.elevate(
        col()
            .key("overlays-popover-card")
            .role("menu")
            .label("Overlays and diagnostics")
            .pin(
                None,
                Some(size::TITLEBAR_H + space::SM),
                Some(space::XXL + 188.0),
                None,
            )
            .w(size::OVERLAYS_POPOVER_W)
            .padding(space::CARD_PAD)
            .gap(space::ROW_GAP)
            .radius(k.radius.lg)
            .continuous_corners()
            .backdrop_blur(size::HUD_BLUR_STRONG)
            .fill(color::popover_bg())
            .border(border::HAIRLINE, k.c.border_strong)
            .child(
                row()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        text("OVERLAYS & DIAGNOSTICS")
                            .font(ty::section_header(k))
                            .color(k.c.text_muted),
                    )
                    .child(
                        text(format!("{} active", s.active_overlay_count()))
                            .font(ty::mono_probe(k))
                            .color(k.c.text_faint),
                    ),
            )
            .child(el().w_full().h(border::HAIRLINE).fill(k.c.border_muted))
            .child(
                col()
                    .w_full()
                    .gap(space::XS)
                    .child(toggle_row(
                        "ov-clipping",
                        "Exposure Clipping",
                        "⌘E",
                        s.camera.exposure_warning,
                        k.c.danger.fg,
                        |s, tx| s.toggle_exposure_warning_tx(tx),
                    ))
                    .child(toggle_row(
                        "ov-focus-mask",
                        "Focus Mask",
                        "⌘M",
                        s.camera.focus_mask,
                        k.c.success.fg,
                        |s, tx| s.toggle_focus_mask_tx(tx),
                    ))
                    .child(toggle_row(
                        "ov-mask-red",
                        "Red Mask Overlay",
                        "M",
                        s.camera.mask_overlay_mode == 1,
                        color::mask_overlay_red(),
                        |s, tx| s.toggle_red_mask_overlay_tx(tx),
                    ))
                    .child(toggle_row(
                        "ov-mask-gray",
                        "Grayscale Mask",
                        "⌥M",
                        s.camera.mask_overlay_mode == 2,
                        color::mask_overlay_gray(),
                        |s, tx| s.toggle_gray_mask_overlay_tx(tx),
                    ))
                    .child(toggle_row(
                        "ov-loupe",
                        "GPU Loupe",
                        "O",
                        s.camera.loupe_enabled,
                        k.c.text_accent,
                        |s, tx| s.toggle_loupe_tx(tx),
                    ))
                    .child(toggle_row(
                        "ov-grid",
                        "Composition Grid",
                        grid_label,
                        grid_active,
                        color::mask_guide_primary(),
                        |s, tx| {
                            let idx = CompositionGrid::ALL
                                .iter()
                                .position(|&g| g == s.recipe.geometry.grid)
                                .unwrap_or(0);
                            let next = CompositionGrid::ALL[(idx + 1) % CompositionGrid::ALL.len()];
                            s.update_geometry_slider_tx(true, tx, |r| r.geometry.grid = next);
                        },
                    )),
            )
            .child(el().w_full().h(border::HAIRLINE).fill(k.c.border_muted))
            .child(
                row()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .px(space::MD)
                    .h(size::PILL_H)
                    .radius(k.radius.md)
                    .hover(k.c.layer_02)
                    .cursor(Cursor::Pointer)
                    .child(
                        text("Proof Margin")
                            .font(ty::control_label(k))
                            .color(k.c.text_muted),
                    )
                    .child(
                        text(format!("{} · ⌘⇧M", s.proof_margin.label()))
                            .font(ty::mono_probe(k))
                            .color(k.c.text_base),
                    )
                    .on_press_tx(|s: &mut AppState, tx| s.cycle_proof_margin_tx(tx)),
            ),
        &k.elevation.overlay,
    );

    scrim_tx("overlays-popover-scrim", |s: &mut AppState, _| {
        s.overlays_popover_open = false;
    })
    .fill(Color::TRANSPARENT)
    .child(card)
}

fn el_v_divider(k: &Kit) -> El {
    row()
        .px(space::XXS)
        .h(size::DIVIDER_TOOLBAR_H)
        .items_center()
        .child(Divider::vertical().build(k))
}
