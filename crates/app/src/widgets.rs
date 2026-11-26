//! Pro Capture One / Lightroom darkroom inspector widgets styled exclusively with
//! centralized Anomaly OpenCode tokens (`crate::tokens`).

use rune::{
    Color, Cursor, Drag, El, Key, KeyEvent, LineCap, Phase, ScrollSpec, Shape, Tx, col, el, icon,
    row, text,
};
use rune_editor_core::{
    ADVANCED_COLOR_SLICE_COUNT, ALL_CLIPBOARD_GROUPS, BlackAndWhiteParams, ClarityMethod,
    ColorEditorSlice, ColorWheel, CompositionGrid, CropAspect, DevelopRecipe, FilmCurve,
    FilmNegativeParams, GrainType, HISTOGRAM_BINS, HSL_BAND_COUNT, HSL_BAND_NAMES, LensParams,
    LevelsParams, ProcessingMode, SkinToneParams, ToneCurves, VignetteMethod, WbPreset,
    default_curve_points, linear_to_srgb, temperature_to_kelvin,
};
use rune_kit::{
    Button, ButtonVariant, Checkbox, ControlSize, IconButton, IconButtonVariant, Kit,
    SegmentedControl, Switch, TextInput,
};
use rune_theme_opencode::icons;

use crate::color_wheel_tex::{
    hsv_to_rgb, pointer_to_wheel_xy, wheel_pixels, wheel_puck_color, wheel_xy_to_point,
};
use crate::state::{
    AppState, ColorBalanceView, ColorEditorMode, CurveChannel, InfoTip, InspectorTab,
    LevelsChannel, descriptor_default, descriptor_range, scalar_default, scalar_range,
};
use crate::tokens::{self, border, color, radius, size, space, ty};

/// Pinned live RGB + Luminance histogram with shadow/highlight clipping telemetry
/// and a fixed-height live cursor `RGB • HSL • Lab` color readout footer.
pub fn histogram_panel(s: &AppState, k: &Kit) -> El {
    let w = size::HISTOGRAM_W;
    let h = size::HISTOGRAM_H;
    let step = w / (HISTOGRAM_BINS.saturating_sub(1).max(1)) as f32;

    let build_area = |bins: &[f32; HISTOGRAM_BINS]| -> (Vec<[f32; 2]>, Vec<[f32; 2]>) {
        let mut line = Vec::with_capacity(HISTOGRAM_BINS);
        let mut poly = Vec::with_capacity(HISTOGRAM_BINS + 2);
        poly.push([0.0, h]);
        for (i, &val) in bins.iter().enumerate() {
            let x = i as f32 * step;
            let y = h - (val.clamp(0.0, 1.0) * (h - space::XS));
            line.push([x, y]);
            poly.push([x, y]);
        }
        poly.push([w, h]);
        (poly, line)
    };

    let (l_poly, l_line) = build_area(&s.histogram.luma);
    let (r_poly, r_line) = build_area(&s.histogram.r);
    let (g_poly, g_line) = build_area(&s.histogram.g);
    let (b_poly, b_line) = build_area(&s.histogram.b);

    let mut shape = Shape::new().view_box(w, h);
    for q in 1..4 {
        let gx = w * (q as f32 * 0.25);
        shape = shape.line(
            [gx, space::XXS],
            [gx, h],
            border::HAIRLINE,
            LineCap::Butt,
            k.c.border_muted,
        );
    }

    shape = shape
        .fill(l_poly, color::hist_luma_fill())
        .fill(r_poly, color::hist_r_fill())
        .fill(g_poly, color::hist_g_fill())
        .fill(b_poly, color::hist_b_fill())
        .stroke(r_line, border::REGULAR, color::hist_r_stroke())
        .stroke(g_line, border::REGULAR, color::hist_g_stroke())
        .stroke(b_line, border::REGULAR, color::hist_b_stroke())
        .stroke(l_line, border::REGULAR, color::hist_luma_stroke());

    let shadow_clip = s.histogram.shadow_clip_pct;
    let highlight_clip = s.histogram.highlight_clip_pct;
    let blk_color = if shadow_clip > 0.5 {
        k.c.info.fg
    } else {
        k.c.text_muted
    };
    let wht_color = if highlight_clip > 0.5 {
        k.c.danger.fg
    } else {
        k.c.text_muted
    };

    let pin_btn = IconButton::new(&icons::SELECT_ELEMENT)
        .variant(if s.readout_picker_active {
            IconButtonVariant::Contrast
        } else {
            IconButtonVariant::GhostMuted
        })
        .control_size(ControlSize::Mini)
        .label("Pin Color Readout Marker")
        .help("Click photo to pin live graded RGB/Lab readout markers (#1–#4)")
        .on_press_tx(|s: &mut AppState, tx| s.toggle_readout_picker_tx(tx))
        .build(k);

    let hist_tip = InfoTip {
        title: "RGB & Luma Histogram",
        shortcut: Some("⌘E"),
        description: "Live 16-bit linear tonal distribution with shadow (BLK) and highlight (WHT) clipping telemetry plus hover RGB/HSL/Lab probe.",
        cli_key: "rd analyze <photo>  ·  MCP: rune_analyze_photo",
        tier: "Tier 0 · Scope",
    };

    let header = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(
            row()
                .items_center()
                .gap(space::SM)
                .child(
                    text("HISTOGRAM")
                        .font(ty::section_header(k))
                        .color(k.c.text_muted),
                )
                .child(info_badge(
                    k,
                    hist_tip,
                    s.hovered_info.is_some_and(|h| h.title == hist_tip.title),
                )),
        )
        .child(
            row()
                .items_center()
                .gap(space::SM)
                .child(
                    text(format!("BLK {shadow_clip:.1}%"))
                        .font(ty::mono_compact(k))
                        .color(blk_color),
                )
                .child(
                    text(format!("WHT {highlight_clip:.1}%"))
                        .font(ty::mono_compact(k))
                        .color(wht_color),
                )
                .child(pin_btn),
        );

    let plot = el()
        .key("histogram-canvas")
        .w_full()
        .h(h)
        .radius(k.radius.sm)
        .shape(shape);

    // Fixed-height 1-line color probe footer so hovering never causes layout jitter.
    let probe_footer = if let Some(c) = s.cursor_readout {
        let swatch = el()
            .size(size::PROBE_SWATCH, size::PROBE_SWATCH)
            .radius(radius::PROBE_SWATCH)
            .fill(tokens::srgb(
                c.rgb8[0] as f32 / 255.0,
                c.rgb8[1] as f32 / 255.0,
                c.rgb8[2] as f32 / 255.0,
            ))
            .border(border::HAIRLINE, k.c.border_strong)
            .a11y_hidden();
        row()
            .key("color-probe-footer")
            .w_full()
            .h(size::PROBE_FOOTER_H)
            .items_center()
            .justify_between()
            .gap(space::XS)
            .child(
                row().items_center().gap(space::XS).child(swatch).child(
                    text(format!(
                        "RGB {:>3} {:>3} {:>3}",
                        c.rgb8[0], c.rgb8[1], c.rgb8[2]
                    ))
                    .font(ty::mono_probe(k))
                    .color(k.c.text_base),
                ),
            )
            .child(
                text(format!(
                    "HSL {:.0}° {:.0} {:.0} · Lab {:.0} {:+.0} {:+.0}",
                    c.hsl[0],
                    c.hsl[1],
                    c.hsl[2],
                    c.lab[0],
                    if c.lab[1].abs() < 0.5 { 0.0 } else { c.lab[1] },
                    if c.lab[2].abs() < 0.5 { 0.0 } else { c.lab[2] }
                ))
                .font(ty::mono_probe(k))
                .color(k.c.text_muted),
            )
    } else {
        row()
            .key("color-probe-footer")
            .w_full()
            .h(size::PROBE_FOOTER_H)
            .items_center()
            .justify_between()
            .child(
                text("RGB · HSL · Lab")
                    .font(ty::mono_micro(k))
                    .color(k.c.text_faint),
            )
            .child(
                text("Hover photo to sample")
                    .font(ty::caption(k))
                    .color(k.c.text_faint),
            )
    };

    let mut card = col()
        .key("histogram-card")
        .w_full()
        .gap(space::ROW_GAP)
        .padding(space::CARD_PAD)
        .radius(k.radius.lg)
        .fill(k.c.bg_deep)
        .border(border::HAIRLINE, k.c.border_base)
        .child(header)
        .child(plot)
        .child(probe_footer);

    if !s.pinned_readouts.is_empty() {
        let mut pins_col = col()
            .key("pinned-readouts-list")
            .w_full()
            .gap(space::XXS)
            .pt(space::XXS)
            .border_sides(
                [border::HAIRLINE, border::NONE, border::NONE, border::NONE],
                k.c.border_muted,
            );
        for (idx, pin) in s.pinned_readouts.iter().enumerate() {
            let live = s.evaluate_pinned_readout(pin);
            let swatch = el()
                .size(size::PROBE_SWATCH, size::PROBE_SWATCH)
                .radius(radius::PROBE_SWATCH)
                .fill(tokens::srgb(
                    live.rgb8[0] as f32 / 255.0,
                    live.rgb8[1] as f32 / 255.0,
                    live.rgb8[2] as f32 / 255.0,
                ))
                .border(border::HAIRLINE, k.c.border_strong)
                .a11y_hidden();
            let pin_row = row()
                .key(format!("pin-readout-row-{idx}"))
                .w_full()
                .items_center()
                .justify_between()
                .gap(space::XS)
                .child(
                    row()
                        .items_center()
                        .gap(space::XS)
                        .child(
                            text(format!("#{}", idx + 1))
                                .font(ty::mono_probe(k))
                                .color(color::pin_marker_ring()),
                        )
                        .child(swatch)
                        .child(
                            text(format!(
                                "{:>3} {:>3} {:>3}",
                                live.rgb8[0], live.rgb8[1], live.rgb8[2]
                            ))
                            .font(ty::mono_probe(k))
                            .color(k.c.text_base),
                        ),
                )
                .child(
                    row()
                        .items_center()
                        .gap(space::XS)
                        .child(
                            text(format!(
                                "Lab {:.0} {:+.0} {:+.0}",
                                live.lab[0],
                                if live.lab[1].abs() < 0.5 {
                                    0.0
                                } else {
                                    live.lab[1]
                                },
                                if live.lab[2].abs() < 0.5 {
                                    0.0
                                } else {
                                    live.lab[2]
                                }
                            ))
                            .font(ty::mono_probe(k))
                            .color(k.c.text_muted),
                        )
                        .child(
                            el().key(format!("pin-rm-{idx}"))
                                .role("button")
                                .label(format!("Remove pinned readout #{}", idx + 1))
                                .cursor(Cursor::Pointer)
                                .px(space::XXS)
                                .child(
                                    icon(&icons::CLOSE, size::ICON_XS)
                                        .color(k.c.text_faint)
                                        .a11y_hidden(),
                                )
                                .on_press(move |s: &mut AppState| {
                                    if idx < s.pinned_readouts.len() {
                                        s.pinned_readouts.remove(idx);
                                    }
                                }),
                        ),
                );
            pins_col = pins_col.child(pin_row);
        }
        card = card.child(pins_col);
    }

    card
}

/// Subtle 0.5pt hairline divider separating a Capture One tool card header from its controls.
fn card_header_divider(k: &Kit) -> El {
    el().w_full()
        .h(border::HAIRLINE)
        .fill(k.c.border_muted)
        .a11y_hidden()
}

/// Reusable `15pt` circular Tier 2 `ⓘ` hover info badge.
/// Hovering updates transient `AppState.hovered_info` (never dirties recipe or undo history).
pub(crate) fn info_badge(k: &Kit, tip: InfoTip, hovered: bool) -> El {
    let sz = size::INFO_BADGE_SIZE;
    row()
        .key(format!("info-badge-{}", tip.title))
        .role("button")
        .label(format!("Info: {}", tip.title))
        .help(tip.description)
        .cursor(Cursor::Default)
        .size(sz, sz)
        .center()
        .radius(radius::circle(sz))
        .fill(if hovered {
            k.c.text_accent.alpha(0.18)
        } else {
            color::info_badge_bg()
        })
        .border(
            border::REGULAR,
            if hovered {
                k.c.text_accent
            } else {
                color::info_badge_border()
            },
        )
        .child(text("i").font(ty::mono_probe(k)).color(if hovered {
            k.c.text_accent
        } else {
            k.c.text_muted
        }))
        .on_hover_change(move |s: &mut AppState, inside, _tx| {
            if inside {
                s.set_hovered_info(tip);
            } else {
                s.clear_hovered_info(tip.title);
            }
        })
}

/// Floating frosted darkroom info card rendered whenever `state.hovered_info` is `Some(...)`.
/// Positioned cleanly at top-right of the photo canvas (or bottom-left of Settings/Export)
/// so it never overlaps macOS traffic lights or clips off-screen.
pub fn floating_info_popover(s: &AppState, k: &Kit) -> Option<El> {
    let tip = s.hovered_info?;

    let mut header = row()
        .w_full()
        .items_center()
        .justify_between()
        .gap(space::SM)
        .child(
            text(tip.title)
                .font(ty::brand_title(k))
                .color(k.c.text_base)
                .truncate(),
        );

    let mut badges = row().items_center().gap(space::XS);
    if let Some(sc) = tip.shortcut {
        badges = badges.child(
            el().px(space::SM)
                .py(space::MICRO)
                .radius(k.radius.xs)
                .fill(k.c.bg_deep)
                .border(border::HAIRLINE, k.c.border_strong)
                .child(text(sc).font(ty::mono_probe(k)).color(k.c.text_base)),
        );
    }
    badges = badges.child(
        el().px(space::SM)
            .py(space::MICRO)
            .radius(k.radius.xs)
            .fill(k.c.layer_02)
            .border(border::HAIRLINE, k.c.border_muted)
            .child(text(tip.tier).font(ty::mono_probe(k)).color(k.c.text_muted)),
    );
    header = header.child(badges);

    let card = col()
        .key("floating-info-popover")
        .role("tooltip")
        .label(tip.title)
        .w(size::INFO_POPOVER_W)
        .padding(space::CARD_PAD)
        .gap(space::ROW_GAP)
        .radius(k.radius.lg)
        .continuous_corners()
        .backdrop_blur(size::HUD_BLUR_STRONG)
        .fill(color::popover_bg())
        .border(border::HAIRLINE, k.c.border_strong)
        .child(header)
        .child(
            text(tip.description)
                .font(ty::caption(k))
                .color(k.c.text_muted)
                .wrap(),
        )
        .child(
            el().w_full()
                .px(space::MD)
                .py(space::XS)
                .radius(k.radius.sm)
                .fill(k.c.bg_deep)
                .border(border::HAIRLINE, k.c.border_muted)
                .child(
                    text(tip.cli_key)
                        .font(ty::mono_probe(k))
                        .color(k.c.text_accent)
                        .wrap(),
                ),
        );

    let pinned = if s.show_settings || s.show_export {
        card.pin(
            Some(size::SETTINGS_SIDEBAR_W + space::MODAL_PAD),
            None,
            None,
            Some(size::STATUS_BAR_H + space::HUD_Y),
        )
    } else {
        let right_offset = if s.show_inspector {
            size::INSPECTOR_W + space::HUD_X
        } else {
            space::HUD_X
        };
        card.pin(
            None,
            Some(size::TITLEBAR_H + space::HUD_Y),
            Some(right_offset),
            None,
        )
    };

    Some(k.elevate(pinned, &k.elevation.floating))
}

/// Reusable inline Tier 1 `Advanced ▸` progressive disclosure toggle row.
/// Displays an accent modified-count pill whenever hidden Tier 1 controls are non-default.
#[allow(clippy::too_many_arguments)]
pub(crate) fn advanced_disclosure_row(
    k: &Kit,
    key: &'static str,
    label: &'static str,
    is_open: bool,
    modified_count: usize,
    total_count: usize,
    on_toggle: fn(&mut AppState),
) -> El {
    let chevron = if is_open {
        &icons::CHEVRON_DOWN
    } else {
        &icons::CHEVRON_RIGHT
    };

    let mut right_el = row().items_center().gap(space::XS);
    if modified_count > 0 {
        right_el = right_el.child(
            row()
                .items_center()
                .gap(space::XS)
                .px(space::SM)
                .h(18.0)
                .radius(radius::circle(18.0))
                .fill(k.c.text_accent.alpha(0.16))
                .border(border::HAIRLINE, k.c.text_accent.alpha(0.45))
                .child(
                    el().size(size::MODIFIED_DOT, size::MODIFIED_DOT)
                        .radius(radius::circle(size::MODIFIED_DOT))
                        .fill(k.c.text_accent)
                        .a11y_hidden(),
                )
                .child(
                    text(format!("{modified_count} active"))
                        .font(ty::mono_probe(k))
                        .color(k.c.text_accent),
                ),
        );
    } else if total_count > 0 {
        right_el = right_el.child(
            text(format!("{total_count}"))
                .font(ty::mono_probe(k))
                .color(k.c.text_faint),
        );
    }

    row()
        .key(key)
        .role("button")
        .label(format!("Toggle {label}"))
        .expanded(is_open)
        .focusable(true)
        .focus_ring(
            border::FOCUS_RING,
            border::FOCUS_OFFSET_TIGHT,
            k.c.border_focus,
        )
        .cursor(Cursor::Pointer)
        .w_full()
        .h(size::PILL_H)
        .px(space::MD)
        .items_center()
        .justify_between()
        .radius(k.radius.md)
        .fill(if is_open { k.c.layer_02 } else { k.c.bg_base })
        .hover(k.c.layer_02)
        .border(
            border::HAIRLINE,
            if is_open || modified_count > 0 {
                k.c.border_base
            } else {
                k.c.border_muted
            },
        )
        .child(
            row()
                .items_center()
                .gap(space::SM)
                .child(
                    icon(chevron, size::ICON_XS)
                        .color(k.c.icon_muted)
                        .a11y_hidden(),
                )
                .child(text(label).font(ty::control_label(k)).color(
                    if is_open || modified_count > 0 {
                        k.c.text_base
                    } else {
                        k.c.text_muted
                    },
                )),
        )
        .child(right_el)
        .on_press(on_toggle)
}

/// Collapsible Capture One / OpenCode `Studio Airy` tool card with disclosure chevron,
/// uppercase title, subtle modified indicator dot, trailing `ⓘ` hover info badge, and
/// compact right-aligned actions.
#[allow(clippy::too_many_arguments)]
pub(crate) fn collapsible_section_card(
    k: &Kit,
    key: &'static str,
    title: &'static str,
    open: bool,
    modified: bool,
    on_toggle: fn(&mut AppState),
    hovered_info: Option<InfoTip>,
    info: InfoTip,
    header_actions: Option<El>,
    content: El,
) -> El {
    let chevron = if open {
        &icons::CHEVRON_DOWN
    } else {
        &icons::CHEVRON_RIGHT
    };

    let mut toggle = row()
        .role("button")
        .label(format!("Toggle {title}"))
        .expanded(open)
        .focusable(true)
        .focus_ring(border::FOCUS_RING, border::FOCUS_OFFSET, k.c.border_focus)
        .radius(k.radius.xs)
        .cursor(Cursor::Pointer)
        .items_center()
        .gap(space::SM)
        .child(
            icon(chevron, size::ICON_SM)
                .color(k.c.icon_muted)
                .a11y_hidden(),
        )
        .child(text(title).font(ty::section_header(k)).color(if modified {
            k.c.text_base
        } else {
            k.c.text_muted
        }));
    if modified {
        toggle = toggle.child(
            el().size(size::MODIFIED_DOT, size::MODIFIED_DOT)
                .radius(radius::circle(size::MODIFIED_DOT))
                .fill(color::tool_modified_dot(k))
                .a11y_hidden(),
        );
    }
    let toggle = toggle.on_press(on_toggle);

    let title_cluster = row()
        .items_center()
        .gap(space::SM)
        .child(toggle)
        .child(info_badge(
            k,
            info,
            hovered_info.is_some_and(|h| h.title == info.title),
        ));

    let mut header = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(title_cluster);
    if let Some(actions) = header_actions {
        header = header.child(actions);
    }

    let mut card = col()
        .key(key)
        .w_full()
        .gap(space::CARD_HEADER_GAP)
        .padding(space::CARD_PAD)
        .radius(k.radius.lg)
        .fill(k.c.layer_01)
        .border(border::HAIRLINE, k.c.border_base)
        .child(header);
    if open {
        card = card.child(card_header_divider(k)).child(content);
    }
    card
}

pub(crate) fn mini_reset_btn(
    k: &Kit,
    label: &'static str,
    disabled: bool,
    on_press: impl Fn(&mut AppState, &mut Tx) + 'static,
) -> El {
    IconButton::new(&icons::OUTLINE_RESET)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Mini)
        .label(label)
        .help(label)
        .disabled(disabled)
        .on_press_tx(on_press)
        .build(k)
}

/// Capture One-style compact `A` (Auto) pill button for tool card headers.
fn mini_auto_btn(
    k: &Kit,
    key: &'static str,
    help: &'static str,
    on_press: impl Fn(&mut AppState, &mut Tx) + 'static,
) -> El {
    row()
        .key(key)
        .role("button")
        .label(help)
        .help(help)
        .focusable(true)
        .focus_ring(
            border::FOCUS_RING,
            border::FOCUS_OFFSET_TIGHT,
            k.c.border_focus,
        )
        .cursor(Cursor::Pointer)
        .size(size::MINI_AUTO_BTN, size::MINI_AUTO_BTN)
        .center()
        .radius(k.radius.sm)
        .fill(k.c.bg_base)
        .hover(k.c.hover)
        .border(border::HAIRLINE, k.c.border_base)
        .child(text("A").font(ty::mono_compact(k)).color(k.c.text_muted))
        .on_press_tx(on_press)
}

/// Maps a normalized `[0, 1]` scrub position into `min..=max` with a subtle
/// magnetic zero detent around `reset_val` for effortless neutral resets.
#[inline]
fn capsule_value_from_norm(norm: f32, min: f32, max: f32, reset_val: f32) -> f32 {
    let span = max - min;
    if !span.is_finite() || span <= 0.0 {
        return min;
    }
    let n = norm.clamp(0.0, 1.0);
    let reset_norm = ((reset_val - min) / span).clamp(0.0, 1.0);
    if reset_norm > 0.02 && reset_norm < 0.98 && (n - reset_norm).abs() < 0.016 {
        return reset_val;
    }
    (min + n * span).clamp(min, max)
}

/// Core token-driven monochrome Scrubbable Capsule slider (no protruding knob;
/// entire row is horizontally scrubbable, double-click resets to `reset_val`).
#[allow(clippy::too_many_arguments)]
fn capsule_slider_el(
    k: &Kit,
    key: String,
    a11y_label: String,
    height: f32,
    width: f32,
    value: f32,
    range: std::ops::RangeInclusive<f32>,
    value_fmt: String,
    reset_val: f32,
    content_row: El,
    refresh_histogram: bool,
    setter: impl Fn(&mut DevelopRecipe, f32) + Copy + 'static,
) -> El {
    let min = *range.start();
    let max = *range.end();
    let span = (max - min).max(1e-6);
    let norm = ((value - min) / span).clamp(0.0, 1.0);
    let reset_norm = ((reset_val - min) / span).clamp(0.0, 1.0);
    let bipolar = reset_norm > 0.02 && reset_norm < 0.98;
    let modified = (value - reset_val).abs() > (span * 0.0025);

    // Bipolar sliders grow strictly from center `reset_norm` (clean empty capsule at 0);
    // unipolar sliders grow cleanly from the left edge (`0.0`).
    let origin_norm = if bipolar { reset_norm } else { 0.0 };
    let fill_left = norm.min(origin_norm) * width;
    let fill_w = ((norm - origin_norm).abs() * width).max(0.0);
    let edge_x = (norm * width - size::CAPSULE_EDGE_W * 0.5)
        .clamp(0.0, (width - size::CAPSULE_EDGE_W).max(0.0));
    let kbd_step = (f64::from(span) / 40.0).max(1e-4);

    let mut capsule = el()
        .key(key)
        .role("slider")
        .label(a11y_label)
        .help("Drag horizontally to adjust • Double-click to reset")
        .range_value(f64::from(value), f64::from(min), f64::from(max))
        .range_step(kbd_step)
        .a11y_value(value_fmt)
        .focusable(true)
        .focus_ring(
            border::FOCUS_RING,
            border::FOCUS_OFFSET_TIGHT,
            k.c.border_focus,
        )
        .cursor(Cursor::ResizeColumn)
        .w(width)
        .h(height)
        .radius(k.radius.lg)
        .fill(color::capsule_bg(k))
        .hover(color::capsule_hover(k))
        .border(border::HAIRLINE, k.c.border_base)
        .clip_rounded(k.radius.lg);

    // Subtle vertical center detent tick for bipolar sliders.
    if bipolar {
        let center_x = (reset_norm * width - size::CAPSULE_ZERO_TICK_W * 0.5).max(0.0);
        capsule = capsule.child(
            el().absolute(center_x, space::XS)
                .size(size::CAPSULE_ZERO_TICK_W, (height - space::MD).max(2.0))
                .fill(color::capsule_zero_tick(k))
                .a11y_hidden(),
        );
    }

    // Internal monochrome fill bar + crisp leading edge highlight when non-empty.
    if fill_w >= 0.75 && (bipolar || modified) {
        capsule = capsule
            .child(
                el().absolute(fill_left, 0.0)
                    .size(fill_w, height)
                    .fill(color::capsule_fill(k))
                    .a11y_hidden(),
            )
            .child(
                el().absolute(edge_x, 0.0)
                    .size(size::CAPSULE_EDGE_W, height)
                    .fill(color::capsule_edge(k))
                    .a11y_hidden(),
            );
    }

    let apply = move |s: &mut AppState, tx: &mut Tx, v: f32, done: bool| {
        if refresh_histogram {
            s.update_slider_tx(done, tx, |r| setter(r, v));
        } else {
            s.update_geometry_slider_tx(done, tx, |r| setter(r, v));
        }
    };

    capsule
        .child(content_row)
        .on_drag(move |s: &mut AppState, d: &mut Drag, tx: &mut Tx| {
            // Native AppKit double-click resets cleanly to default value.
            if d.clicks >= 2 {
                if d.phase == Phase::Down {
                    apply(s, tx, reset_val, true);
                }
                return;
            }
            let w = d.w.max(1.0);
            let next = capsule_value_from_norm(d.x / w, min, max, reset_val);
            match d.phase {
                Phase::Down | Phase::Move => apply(s, tx, next, false),
                Phase::Up => apply(s, tx, next, true),
            }
        })
        .on_key(move |s: &mut AppState, ev: &KeyEvent, tx: &mut Tx| {
            let step = if ev.mods.shift {
                span * 0.1
            } else {
                span * 0.025
            };
            match &ev.key {
                Key::Left | Key::Down => {
                    apply(s, tx, (value - step).clamp(min, max), true);
                    true
                }
                Key::Right | Key::Up => {
                    apply(s, tx, (value + step).clamp(min, max), true);
                    true
                }
                Key::Backspace | Key::Delete => {
                    apply(s, tx, reset_val, true);
                    true
                }
                Key::Home => {
                    apply(s, tx, min, true);
                    true
                }
                Key::End => {
                    apply(s, tx, max, true);
                    true
                }
                _ => false,
            }
        })
}

/// Unified OpenCode Scrubbable Capsule photographic slider with tabular mono readout.
#[allow(clippy::too_many_arguments)]
pub(crate) fn develop_slider_inner(
    k: &Kit,
    key: impl Into<String>,
    label: &'static str,
    dot_color: Option<Color>,
    value: f32,
    range: std::ops::RangeInclusive<f32>,
    value_fmt: String,
    reset_val: f32,
    refresh_histogram: bool,
    setter: impl Fn(&mut DevelopRecipe, f32) + Copy + 'static,
) -> El {
    develop_slider_sized(
        k,
        key,
        label,
        dot_color,
        size::CARD_INNER_W,
        value,
        range,
        value_fmt,
        reset_val,
        refresh_histogram,
        setter,
    )
}

/// Width-parameterized Scrubbable Capsule slider for nested sub-cards (e.g. Combine Masks).
#[allow(clippy::too_many_arguments)]
pub(crate) fn develop_slider_sized(
    k: &Kit,
    key: impl Into<String>,
    label: &'static str,
    dot_color: Option<Color>,
    width: f32,
    value: f32,
    range: std::ops::RangeInclusive<f32>,
    value_fmt: String,
    reset_val: f32,
    refresh_histogram: bool,
    setter: impl Fn(&mut DevelopRecipe, f32) + Copy + 'static,
) -> El {
    let key_str = key.into();
    let modified = (value - reset_val).abs() > 0.005;

    let mut label_row = row().items_center().gap(space::SM);
    if let Some(col_dot) = dot_color {
        label_row = label_row.child(
            el().size(size::CHANNEL_DOT, size::CHANNEL_DOT)
                .radius(radius::circle(size::CHANNEL_DOT))
                .fill(col_dot)
                .a11y_hidden(),
        );
    }
    label_row = label_row.child(text(label).font(ty::control_label(k)).color(if modified {
        k.c.text_base
    } else {
        k.c.text_muted
    }));

    let content_row = row()
        .w_full()
        .h_full()
        .px(space::XL)
        .items_center()
        .justify_between()
        .child(label_row)
        .child(text(&value_fmt).font(ty::mono_value(k)).color(if modified {
            k.c.text_base
        } else {
            k.c.text_muted
        }));

    capsule_slider_el(
        k,
        key_str,
        label.to_owned(),
        size::CAPSULE_SLIDER_H,
        width,
        value,
        range,
        value_fmt,
        reset_val,
        content_row,
        refresh_histogram,
        setter,
    )
}

/// Registry-backed Scrubbable Capsule slider whose bounds and default come directly from
/// `rune_editor_core::SCALAR_PARAMS`.
#[allow(clippy::too_many_arguments)]
fn develop_scalar_slider(
    k: &Kit,
    key: &'static str,
    param_id: &'static str,
    label: &'static str,
    value: f32,
    value_fmt: String,
    setter: fn(&mut DevelopRecipe, f32),
) -> El {
    develop_slider_inner(
        k,
        key,
        label,
        None,
        value,
        scalar_range(param_id),
        value_fmt,
        scalar_default(param_id),
        true,
        setter,
    )
}

fn channel_color(ch: CurveChannel, k: &Kit) -> Color {
    match ch {
        CurveChannel::Rgb => color::channel_rgb(),
        CurveChannel::Luma => k.c.text_base,
        CurveChannel::Red => color::channel_red(),
        CurveChannel::Green => color::channel_green(),
        CurveChannel::Blue => color::channel_blue(),
    }
}

fn levels_channel_dot(ch: LevelsChannel, k: &Kit) -> Color {
    match ch {
        LevelsChannel::Rgb => k.c.text_base,
        LevelsChannel::Red => color::channel_red(),
        LevelsChannel::Green => color::channel_green(),
        LevelsChannel::Blue => color::channel_blue(),
    }
}

/// Interactive Capture One-style Levels card with `RGB | R | G | B` channel tabs,
/// per-channel colored histogram overlay, shadow/midtone/highlight ticks, Auto Levels (`A`),
/// and Input/Output Scrubbable Capsules.
fn levels_section(s: &AppState, k: &Kit) -> El {
    let lv_all = s.recipe.levels.sanitized();
    let active_ch = s.levels_channel;
    let ch_lv = active_ch.channel(&lv_all);
    let w = size::CARD_INNER_W;
    let h = size::LEVELS_STRIP_H;
    let step = w / (HISTOGRAM_BINS.saturating_sub(1).max(1)) as f32;

    let (bins, fill_col, stroke_col) = match active_ch {
        LevelsChannel::Rgb => (
            &s.histogram.luma,
            color::levels_hist_fill(k),
            color::levels_hist_stroke(k),
        ),
        LevelsChannel::Red => (&s.histogram.r, color::hist_r_fill(), color::hist_r_stroke()),
        LevelsChannel::Green => (&s.histogram.g, color::hist_g_fill(), color::hist_g_stroke()),
        LevelsChannel::Blue => (&s.histogram.b, color::hist_b_fill(), color::hist_b_stroke()),
    };

    let mut line = Vec::with_capacity(HISTOGRAM_BINS);
    let mut poly = Vec::with_capacity(HISTOGRAM_BINS + 2);
    poly.push([0.0, h]);
    for (i, &val) in bins.iter().enumerate() {
        let pt = [i as f32 * step, h - val.clamp(0.0, 1.0) * (h - space::XS)];
        line.push(pt);
        poly.push(pt);
    }
    poly.push([w, h]);

    let bx = ch_lv.in_black * w;
    let wx = ch_lv.in_white * w;
    let mx = bx + (wx - bx) * (0.5 - ch_lv.in_mid * 0.35).clamp(0.05, 0.95);

    let shape = Shape::new()
        .view_box(w, h)
        .fill(poly, fill_col)
        .stroke(line, border::REGULAR, stroke_col)
        .line(
            [bx, 0.0],
            [bx, h],
            border::ACTIVE,
            LineCap::Butt,
            k.c.info.fg,
        )
        .line(
            [mx, space::XXS],
            [mx, h],
            border::REGULAR,
            LineCap::Butt,
            k.c.text_muted,
        )
        .line(
            [wx, 0.0],
            [wx, h],
            border::ACTIVE,
            LineCap::Butt,
            k.c.text_accent,
        );

    let strip = el()
        .key("levels-strip")
        .w(w)
        .h(h)
        .radius(k.radius.sm)
        .fill(k.c.bg_deep)
        .border(border::HAIRLINE, k.c.border_base)
        .shape(shape);

    // Channel switcher (`RGB | R | G | B`) with active dot and per-channel modified cue.
    let mut ch_row = row()
        .key("levels-channels")
        .role("radiogroup")
        .label("Levels channel")
        .w_full()
        .gap(space::XS);
    for ch_opt in LevelsChannel::ALL {
        let active = ch_opt == active_ch;
        let ch_modified = ch_opt.is_modified(&lv_all);
        let dot_col = levels_channel_dot(ch_opt, k);
        let mut pill = row()
            .key(format!("lvl-ch-{}", ch_opt.short_label()))
            .role("radio")
            .label(format!("{} levels channel", ch_opt.label()))
            .checked(active)
            .focusable(true)
            .focus_ring(
                border::FOCUS_RING,
                border::FOCUS_OFFSET_TIGHT,
                k.c.border_focus,
            )
            .cursor(Cursor::Pointer)
            .grow(1.0)
            .min_w(0.0)
            .h(size::PILL_H)
            .center()
            .gap(space::TIGHT)
            .radius(k.radius.sm)
            .fill(color::pill_fill(k, active))
            .hover(k.c.hover)
            .border(
                if active {
                    border::REGULAR
                } else {
                    border::HAIRLINE
                },
                if active {
                    k.c.border_strong
                } else {
                    k.c.border_muted
                },
            )
            .child(
                el().size(size::CHANNEL_DOT, size::CHANNEL_DOT)
                    .radius(radius::circle(size::CHANNEL_DOT))
                    .fill(dot_col)
                    .a11y_hidden(),
            )
            .child(
                text(ch_opt.short_label())
                    .font(ty::section_header(k))
                    .color(if active {
                        k.c.text_base
                    } else {
                        k.c.text_muted
                    }),
            );
        if ch_modified {
            pill = pill.child(
                el().pin(Some(space::XL), None, Some(space::XL), Some(space::MICRO))
                    .h(space::XXS)
                    .radius(space::MICRO)
                    .fill(color::levels_modified_bar())
                    .a11y_hidden(),
            );
        }
        ch_row = ch_row.child(pill.on_press(move |s: &mut AppState| {
            s.levels_channel = ch_opt;
        }));
    }

    let ch_tag = active_ch.short_label();
    let out_mod_count =
        usize::from(ch_lv.out_black > 0.005) + usize::from((1.0 - ch_lv.out_white).abs() > 0.005);

    let mut controls = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(ch_row)
        .child(strip)
        .child(develop_slider_inner(
            k,
            format!("sl-lvl-{ch_tag}-in-black"),
            "In Shadow",
            None,
            ch_lv.in_black,
            0.0..=0.95,
            format!("{:.0}", ch_lv.in_black * 255.0),
            0.0,
            true,
            move |rec, v| active_ch.set_in_black(&mut rec.levels, v),
        ))
        .child(develop_slider_inner(
            k,
            format!("sl-lvl-{ch_tag}-in-mid"),
            "In Midtone",
            None,
            ch_lv.in_mid,
            -1.0..=1.0,
            format!("{:+.0}", ch_lv.in_mid * 100.0),
            0.0,
            true,
            move |rec, v| active_ch.set_in_mid(&mut rec.levels, v),
        ))
        .child(develop_slider_inner(
            k,
            format!("sl-lvl-{ch_tag}-in-white"),
            "In Highlight",
            None,
            ch_lv.in_white,
            0.05..=1.0,
            format!("{:.0}", ch_lv.in_white * 255.0),
            1.0,
            true,
            move |rec, v| active_ch.set_in_white(&mut rec.levels, v),
        ))
        .child(advanced_disclosure_row(
            k,
            "adv-levels-output",
            "Output Levels",
            s.levels_advanced_open,
            out_mod_count,
            2,
            |s| s.levels_advanced_open = !s.levels_advanced_open,
        ));

    if s.levels_advanced_open {
        controls = controls
            .child(develop_slider_inner(
                k,
                format!("sl-lvl-{ch_tag}-out-black"),
                "Out Shadow",
                None,
                ch_lv.out_black,
                0.0..=1.0,
                format!("{:.0}", ch_lv.out_black * 255.0),
                0.0,
                true,
                move |rec, v| active_ch.set_out_black(&mut rec.levels, v),
            ))
            .child(develop_slider_inner(
                k,
                format!("sl-lvl-{ch_tag}-out-white"),
                "Out Highlight",
                None,
                ch_lv.out_white,
                0.0..=1.0,
                format!("{:.0}", ch_lv.out_white * 255.0),
                1.0,
                true,
                move |rec, v| active_ch.set_out_white(&mut rec.levels, v),
            ));
    }

    let actions = row()
        .items_center()
        .gap(space::XS)
        .child(mini_auto_btn(
            k,
            "btn-auto-levels",
            "Auto Levels (0.1% clip)",
            |s, tx| s.auto_levels_tx(tx),
        ))
        .child(mini_reset_btn(
            k,
            "Reset Levels",
            lv_all.is_identity(),
            |s, tx| {
                s.update_slider_tx(true, tx, |rec| rec.levels = LevelsParams::default());
            },
        ));

    collapsible_section_card(
        k,
        "sec-levels",
        "LEVELS",
        s.levels_open,
        !lv_all.is_identity(),
        |s| s.levels_open = !s.levels_open,
        s.hovered_info,
        InfoTip {
            title: "Per-Channel Levels",
            shortcut: None,
            description: "Remaps black, midtone gamma, and white points across RGB or individual R/G/B channels. Click A for 0.1% Auto Levels.",
            cli_key: "levels.{in_black,in_mid,in_white,out_black,out_white}",
            tier: "Tier 1 · Levels",
        },
        Some(actions),
        controls,
    )
}

/// Interactive Tone Curve Editor with Fritsch-Carlson monotonic cubic spline preview,
/// Luma / R / G / B channel switcher, background histogram silhouette, and draggable handles.
fn tone_curve_editor(s: &AppState, k: &Kit) -> El {
    let cw = size::CARD_INNER_W;
    let ch = size::CURVE_H;
    let active_ch = s.curve_channel;
    let active_pts = active_ch.points(&s.recipe.curves);
    let active_color = channel_color(active_ch, k);

    let mut shape = Shape::new().view_box(cw, ch);

    let step = cw / (HISTOGRAM_BINS.saturating_sub(1).max(1)) as f32;
    let mut hist_poly = Vec::with_capacity(HISTOGRAM_BINS + 2);
    hist_poly.push([0.0, ch]);
    for (i, &val) in s.histogram.luma.iter().enumerate() {
        hist_poly.push([i as f32 * step, ch - val.clamp(0.0, 1.0) * (ch * 0.72)]);
    }
    hist_poly.push([cw, ch]);
    shape = shape.fill(hist_poly, color::curve_bg_hist(k));

    for i in 1..4 {
        let f = i as f32 * 0.25;
        shape = shape
            .line(
                [cw * f, 0.0],
                [cw * f, ch],
                border::HAIRLINE,
                LineCap::Butt,
                k.c.border_muted,
            )
            .line(
                [0.0, ch * f],
                [cw, ch * f],
                border::HAIRLINE,
                LineCap::Butt,
                k.c.border_muted,
            );
    }

    shape = shape.line(
        [0.0, ch],
        [cw, 0.0],
        border::THIN,
        LineCap::Butt,
        k.c.border_base,
    );

    for other in CurveChannel::ALL {
        if other == active_ch {
            continue;
        }
        let pts = other.points(&s.recipe.curves);
        if pts.len() == 2 && pts[0] == [0.0, 0.0] && pts[1] == [1.0, 1.0] {
            continue;
        }
        let sampled: Vec<[f32; 2]> = (0..=48)
            .map(|i| {
                let x = i as f32 / 48.0;
                let y = ToneCurves::evaluate_channel(pts, x);
                [x * cw, (1.0 - y) * ch]
            })
            .collect();
        shape = shape.stroke(
            sampled,
            border::CURVE_GHOST,
            color::curve_ghost(channel_color(other, k)),
        );
    }

    let spline_pts: Vec<[f32; 2]> = (0..=64)
        .map(|i| {
            let x = i as f32 / 64.0;
            let y = ToneCurves::evaluate_channel(active_pts, x);
            [x * cw, (1.0 - y) * ch]
        })
        .collect();
    shape = shape.stroke(spline_pts, border::CURVE_ACTIVE, active_color);

    // Capture One Pro live orange vertical tonal indicator tracking hovered canvas pixel luminance.
    let probe_overlay = s.cursor_readout.map(|readout| {
        let px_norm = active_ch.probe_tone(&readout);
        let py_norm = ToneCurves::evaluate_channel(active_pts, px_norm).clamp(0.0, 1.0);
        let ix = (px_norm * cw).clamp(1.0, cw - 1.0);
        let iy = ((1.0 - py_norm) * ch).clamp(1.0, ch - 1.0);
        (ix, iy)
    });
    if let Some((ix, _)) = probe_overlay {
        shape = shape.line(
            [ix, 0.0],
            [ix, ch],
            border::ACTIVE,
            LineCap::Butt,
            color::curve_tonal_indicator_line(),
        );
    }

    let mut canvas_box = el()
        .key("tone-curve-canvas")
        .role("group")
        .label(format!("{} tone curve graph", active_ch.label()))
        .w(cw)
        .h(ch)
        .radius(k.radius.md)
        .fill(k.c.bg_deep)
        .border(border::HAIRLINE, k.c.border_base)
        .cursor(Cursor::Crosshair)
        .shape(shape)
        .on_drag(move |s: &mut AppState, d: &mut Drag, tx: &mut Tx| {
            let nx = (d.x / d.w.max(1.0)).clamp(0.0, 1.0);
            let ny = (1.0 - d.y / d.h.max(1.0)).clamp(0.0, 1.0);
            let ch_sel = s.curve_channel;

            match d.phase {
                Phase::Down => {
                    if d.clicks >= 2 {
                        let pts = ch_sel.points(&s.recipe.curves);
                        let mut best: Option<(usize, f32)> = None;
                        for (idx, p) in pts.iter().enumerate() {
                            let px = p[0] * d.w;
                            let py = (1.0 - p[1]) * d.h;
                            let dist = (px - d.x).hypot(py - d.y);
                            if dist <= 14.0 && best.is_none_or(|(_, bd)| dist < bd) {
                                best = Some((idx, dist));
                            }
                        }
                        let remove_idx = best
                            .map(|(idx, _)| idx)
                            .filter(|&idx| idx > 0 && idx + 1 < pts.len());
                        if let Some(idx) = remove_idx {
                            let prev = s
                                .drag_undo_snapshot
                                .take()
                                .unwrap_or_else(|| s.recipe.clone());
                            ch_sel.points_mut(&mut s.recipe.curves).remove(idx);
                            s.dragging_curve_pt = None;
                            s.recipe.preset_name = None;
                            s.catalog.update_active_recipe(s.recipe.clone());
                            if prev != s.recipe {
                                s.history.push(prev);
                            }
                            s.recompute_histogram();
                            s.sync_window_state(tx);
                        } else {
                            s.dragging_curve_pt = None;
                        }
                        return;
                    }
                    if s.drag_undo_snapshot.is_none() {
                        s.drag_undo_snapshot = Some(s.recipe.clone());
                    }
                    let pts = ch_sel.points_mut(&mut s.recipe.curves);
                    let mut best: Option<(usize, f32)> = None;
                    for (idx, p) in pts.iter().enumerate() {
                        let px = p[0] * d.w;
                        let py = (1.0 - p[1]) * d.h;
                        let dist = (px - d.x).hypot(py - d.y);
                        if dist <= 14.0 && best.is_none_or(|(_, bd)| dist < bd) {
                            best = Some((idx, dist));
                        }
                    }
                    let grabbed = if let Some((idx, _)) = best {
                        Some(idx)
                    } else if pts.len() < 12 {
                        let nx_ins = nx.clamp(0.025, 0.975);
                        let insert_at = pts
                            .partition_point(|p| p[0] < nx_ins)
                            .clamp(1, pts.len().saturating_sub(1));
                        pts.insert(insert_at, [nx_ins, ny]);
                        Some(insert_at)
                    } else {
                        None
                    };
                    if let Some(idx) = grabbed {
                        s.dragging_curve_pt = Some(idx);
                        s.recipe.preset_name = None;
                        s.recompute_histogram();
                    } else {
                        s.dragging_curve_pt = None;
                        s.drag_undo_snapshot = None;
                    }
                }
                Phase::Move => {
                    if d.clicks >= 2 {
                        return;
                    }
                    if let Some(idx) = s.dragging_curve_pt {
                        let pts = ch_sel.points_mut(&mut s.recipe.curves);
                        if idx < pts.len() {
                            let last = pts.len().saturating_sub(1);
                            let clamped_x = if idx == 0 {
                                0.0
                            } else if idx == last {
                                1.0
                            } else {
                                let min_x = pts[idx - 1][0] + 0.025;
                                let max_x = pts[idx + 1][0] - 0.025;
                                nx.clamp(min_x, max_x.max(min_x))
                            };
                            pts[idx] = [clamped_x, ny];
                            s.recipe.preset_name = None;
                            s.recompute_histogram();
                            tx.instant();
                        }
                    }
                }
                Phase::Up => {
                    if d.clicks >= 2 {
                        s.dragging_curve_pt = None;
                        return;
                    }
                    s.dragging_curve_pt = None;
                    s.catalog.update_active_recipe(s.recipe.clone());
                    if let Some(prev) = s.drag_undo_snapshot.take()
                        && prev != s.recipe
                    {
                        s.history.push(prev);
                    }
                    s.sync_window_state(tx);
                }
            }
        });

    for (idx, &[px, py]) in active_pts.iter().enumerate() {
        let is_dragged = s.dragging_curve_pt == Some(idx);
        let pt_sz = if is_dragged {
            size::CURVE_PT_DRAG
        } else {
            size::CURVE_PT
        };
        let cx = (px * cw - pt_sz * 0.5).clamp(space::MICRO, cw - pt_sz - space::MICRO);
        let cy = ((1.0 - py) * ch - pt_sz * 0.5).clamp(space::MICRO, ch - pt_sz - space::MICRO);
        canvas_box = canvas_box.child(
            el().key(format!("curve-pt-{idx}"))
                .absolute(cx, cy)
                .size(pt_sz, pt_sz)
                .radius(radius::circle(pt_sz))
                .fill(if is_dragged {
                    active_color
                } else {
                    k.c.bg_base
                })
                .border(border::ACTIVE, active_color)
                .a11y_hidden(),
        );
    }

    if let Some((ix, iy)) = probe_overlay {
        let d_sz = size::CURVE_PROBE_DOT;
        canvas_box = canvas_box.child(
            el().key("curve-tonal-probe-dot")
                .absolute(
                    (ix - d_sz * 0.5).clamp(space::MICRO, cw - d_sz - space::MICRO),
                    (iy - d_sz * 0.5).clamp(space::MICRO, ch - d_sz - space::MICRO),
                )
                .size(d_sz, d_sz)
                .radius(radius::circle(d_sz))
                .fill(color::curve_tonal_indicator())
                .border(border::REGULAR, k.c.bg_base)
                .a11y_hidden(),
        );
    }

    let mut ch_row = row()
        .role("radiogroup")
        .label("Tone curve channel")
        .items_center()
        .gap(space::TINY);
    for ch_opt in CurveChannel::ALL {
        let active = ch_opt == active_ch;
        let dot_col = channel_color(ch_opt, k);
        let pill = row()
            .key(format!("curve-ch-{}", ch_opt.short_label()))
            .role("radio")
            .label(format!("{} curve channel", ch_opt.label()))
            .checked(active)
            .focusable(true)
            .focus_ring(
                border::FOCUS_RING,
                border::FOCUS_OFFSET_TIGHT,
                k.c.border_focus,
            )
            .cursor(Cursor::Pointer)
            .items_center()
            .gap(space::XS)
            .h(size::PILL_H)
            .px(space::SM)
            .radius(k.radius.sm)
            .fill(color::pill_fill(k, active))
            .hover(k.c.hover)
            .border(
                if active {
                    border::REGULAR
                } else {
                    border::HAIRLINE
                },
                if active {
                    k.c.border_strong
                } else {
                    k.c.border_muted
                },
            )
            .child(
                el().size(size::CHANNEL_DOT, size::CHANNEL_DOT)
                    .radius(radius::circle(size::CHANNEL_DOT))
                    .fill(dot_col)
                    .a11y_hidden(),
            )
            .child(
                text(ch_opt.short_label())
                    .font(ty::section_header(k))
                    .color(if active {
                        k.c.text_base
                    } else {
                        k.c.text_muted
                    }),
            )
            .on_press(move |s: &mut AppState| {
                s.curve_channel = ch_opt;
                s.dragging_curve_pt = None;
            });
        ch_row = ch_row.child(pill);
    }

    let channel_is_flat = active_pts == default_curve_points().as_slice();
    let any_curve_modified = !s.recipe.curves.is_identity();
    let controls_row = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(ch_row)
        .child(
            text(format!("{} pts", active_pts.len()))
                .font(ty::mono_compact(k))
                .color(k.c.text_faint),
        );

    let body = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(controls_row)
        .child(canvas_box);

    let reset = mini_reset_btn(k, "Reset active curve channel", channel_is_flat, |s, tx| {
        s.reset_active_curve_channel_tx(tx);
    });

    collapsible_section_card(
        k,
        "sec-tone-curve",
        "TONE CURVE",
        s.curve_open,
        any_curve_modified,
        |s| s.curve_open = !s.curve_open,
        s.hovered_info,
        InfoTip {
            title: "5-Tab Tone Curve",
            shortcut: None,
            description: "Fritsch-Carlson monotonic cubic spline across RGB, Luma, and R/G/B channels. Click graph to add a point, drag to shape, double-click to remove.",
            cli_key: "curves.{rgb,luma,red,green,blue}  ·  MCP: rune_edit_photo",
            tier: "Tier 1 · Spline",
        },
        Some(reset),
        body,
    )
}

#[derive(Clone, Copy)]
enum WheelSlot {
    Master,
    Shadows,
    Midtones,
    Highlights,
}

impl WheelSlot {
    fn title(self) -> &'static str {
        match self {
            Self::Master => "Master",
            Self::Shadows => "Shadows",
            Self::Midtones => "Midtones",
            Self::Highlights => "Highlights",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Master => "wheel-master",
            Self::Shadows => "wheel-shadows",
            Self::Midtones => "wheel-midtones",
            Self::Highlights => "wheel-highlights",
        }
    }

    fn get(self, r: &DevelopRecipe) -> ColorWheel {
        match self {
            Self::Master => r.master_wheel,
            Self::Shadows => r.shadows_wheel,
            Self::Midtones => r.midtones_wheel,
            Self::Highlights => r.highlights_wheel,
        }
    }

    fn get_mut(self, r: &mut DevelopRecipe) -> &mut ColorWheel {
        match self {
            Self::Master => &mut r.master_wheel,
            Self::Shadows => &mut r.shadows_wheel,
            Self::Midtones => &mut r.midtones_wheel,
            Self::Highlights => &mut r.highlights_wheel,
        }
    }
}

fn color_wheel_disc(k: &Kit, slot: WheelSlot, wheel: ColorWheel, diameter: f32) -> El {
    let puck_pt = wheel_xy_to_point(wheel.hue_x, wheel.hue_y, diameter);
    let center = [diameter * 0.5, diameter * 0.5];
    let arm = size::WHEEL_CROSSHAIR_ARM;
    let modified = !wheel.is_neutral();

    let mut overlay = Shape::new()
        .view_box(diameter, diameter)
        .line(
            [center[0] - arm, center[1]],
            [center[0] + arm, center[1]],
            border::THIN,
            LineCap::Round,
            color::wheel_crosshair(k),
        )
        .line(
            [center[0], center[1] - arm],
            [center[0], center[1] + arm],
            border::THIN,
            LineCap::Round,
            color::wheel_crosshair(k),
        );

    if wheel.hue_x.hypot(wheel.hue_y) > 0.02 {
        overlay = overlay.line(
            center,
            puck_pt,
            border::CURVE_GHOST,
            LineCap::Round,
            color::wheel_tether(k),
        );
    }

    let puck_size = size::WHEEL_PUCK;
    let puck = el()
        .key(format!("{}-puck", slot.key()))
        .absolute(puck_pt[0] - puck_size * 0.5, puck_pt[1] - puck_size * 0.5)
        .size(puck_size, puck_size)
        .radius(radius::circle(puck_size))
        .fill(wheel_puck_color(wheel.hue_x, wheel.hue_y))
        .border(border::ACTIVE, k.c.white)
        .shadow(
            border::PUCK_SHADOW_Y,
            border::PUCK_SHADOW_BLUR,
            color::puck_shadow(k),
        )
        .a11y_hidden();

    el().key(format!("{}-disc", slot.key()))
        .role("slider")
        .label(format!("{} color balance wheel", slot.title()))
        .help(format!("Drag to tint {} hue and chroma", slot.title()))
        .focusable(true)
        .focus_ring(border::FOCUS_RING, border::FOCUS_OFFSET, k.c.border_focus)
        .size(diameter, diameter)
        .radius(radius::circle(diameter))
        .pixels(wheel_pixels())
        .border(
            border::REGULAR,
            if modified {
                k.c.border_strong
            } else {
                k.c.border_base
            },
        )
        .shape(overlay)
        .cursor(Cursor::Crosshair)
        .child(puck)
        .on_drag(move |s: &mut AppState, d: &mut Drag, tx: &mut Tx| {
            let (hx, hy) = pointer_to_wheel_xy(d.x, d.y, d.w.max(1.0));
            let done = matches!(d.phase, Phase::Up);
            if matches!(d.phase, Phase::Move) {
                tx.instant();
            }
            s.update_slider_tx(done, tx, |r| {
                let w = slot.get_mut(r);
                w.hue_x = hx;
                w.hue_y = hy;
            });
        })
}

fn single_color_wheel(s: &AppState, k: &Kit, slot: WheelSlot) -> El {
    let diameter = size::WHEEL_DIAMETER;
    let wheel = slot.get(&s.recipe);
    let modified = !wheel.is_neutral();
    let disc = color_wheel_disc(k, slot, wheel, diameter);

    let title_btn = row()
        .role("button")
        .label(format!("Reset {} wheel", slot.title()))
        .help(format!("Click to reset {} color wheel", slot.title()))
        .focusable(true)
        .focus_ring(
            border::FOCUS_RING,
            border::FOCUS_OFFSET_TIGHT,
            k.c.border_focus,
        )
        .cursor(Cursor::Pointer)
        .items_center()
        .gap(space::XS)
        .child(
            text(slot.title())
                .font(ty::section_header(k))
                .color(if modified {
                    k.c.text_base
                } else {
                    k.c.text_muted
                }),
        )
        .on_press_tx(move |s: &mut AppState, tx| {
            s.update_slider_tx(true, tx, |r| {
                *slot.get_mut(r) = ColorWheel::default();
            });
        });

    let luma_fmt = format!("Y {:+.2}", wheel.luma);
    let luma_modified = wheel.luma.abs() > 0.005;
    let luma_content =
        row()
            .w_full()
            .h_full()
            .center()
            .child(
                text(&luma_fmt)
                    .font(ty::mono_micro(k))
                    .color(if luma_modified {
                        k.c.text_base
                    } else {
                        k.c.text_muted
                    }),
            );

    let luma_slider = capsule_slider_el(
        k,
        format!("{}-luma", slot.key()),
        format!("{} luminance", slot.title()),
        size::MINI_CAPSULE_SLIDER_H,
        diameter,
        wheel.luma,
        -1.0..=1.0,
        luma_fmt,
        0.0,
        luma_content,
        true,
        move |r, v| {
            slot.get_mut(r).luma = v;
        },
    );

    col()
        .key(slot.key())
        .w(diameter)
        .items_center()
        .gap(space::SM)
        .child(title_btn)
        .child(disc)
        .child(luma_slider)
}

/// Full-size interactive Master Color Balance wheel + global Lightness capsule.
fn master_color_wheel_view(s: &AppState, k: &Kit) -> El {
    let wheel = s.recipe.master_wheel;
    let disc = color_wheel_disc(k, WheelSlot::Master, wheel, size::MASTER_WHEEL_DIAMETER);
    let mag = wheel.hue_x.hypot(wheel.hue_y).clamp(0.0, 1.0);
    let telemetry = if mag > 0.01 {
        let deg = wheel
            .hue_y
            .atan2(wheel.hue_x)
            .rem_euclid(std::f32::consts::TAU)
            .to_degrees();
        format!("Hue {deg:.0}°  ·  Sat {:.0}%", mag * 100.0)
    } else {
        "Neutral  ·  Drag wheel to tint".to_owned()
    };

    let luma_slider = develop_slider_inner(
        k,
        "wheel-master-luma",
        "Lightness",
        None,
        wheel.luma,
        -1.0..=1.0,
        format!("Y {:+.2}", wheel.luma),
        0.0,
        true,
        |r, v| r.master_wheel.luma = v,
    );

    col()
        .key("color-balance-master")
        .w_full()
        .items_center()
        .gap(space::MD)
        .child(disc)
        .child(
            text(telemetry)
                .font(ty::mono_compact(k))
                .color(if mag > 0.01 {
                    k.c.text_base
                } else {
                    k.c.text_faint
                }),
        )
        .child(luma_slider)
}

/// Capture One-style Color Balance card with `3-Way | Master` view switcher.
fn color_wheels_section(s: &AppState, k: &Kit) -> El {
    let three_way_modified = !s.recipe.shadows_wheel.is_neutral()
        || !s.recipe.midtones_wheel.is_neutral()
        || !s.recipe.highlights_wheel.is_neutral();
    let master_modified = !s.recipe.master_wheel.is_neutral();
    let all_neutral = !three_way_modified && !master_modified;

    let mode_tab = |view: ColorBalanceView, label: &'static str, modified: bool| {
        let active = s.color_balance_view == view;
        let mut pill = row()
            .key(format!("cb-mode-{label}"))
            .role("radio")
            .label(format!("{label} color balance view"))
            .checked(active)
            .focusable(true)
            .focus_ring(
                border::FOCUS_RING,
                border::FOCUS_OFFSET_TIGHT,
                k.c.border_focus,
            )
            .cursor(Cursor::Pointer)
            .grow(1.0)
            .min_w(0.0)
            .h(size::PILL_H)
            .center()
            .gap(space::TIGHT)
            .radius(k.radius.sm)
            .fill(color::pill_fill(k, active))
            .hover(k.c.hover)
            .border(
                if active {
                    border::REGULAR
                } else {
                    border::HAIRLINE
                },
                if active {
                    k.c.border_strong
                } else {
                    k.c.border_muted
                },
            )
            .child(text(label).font(ty::section_header(k)).color(if active {
                k.c.text_base
            } else {
                k.c.text_muted
            }));
        if modified {
            pill = pill.child(
                el().size(size::MODIFIED_DOT, size::MODIFIED_DOT)
                    .radius(radius::circle(size::MODIFIED_DOT))
                    .fill(color::tool_modified_dot(k))
                    .a11y_hidden(),
            );
        }
        pill.on_press(move |s: &mut AppState| {
            s.color_balance_view = view;
        })
    };

    let mode_switcher = row()
        .key("cb-mode-switcher")
        .role("radiogroup")
        .label("Color balance mode")
        .w_full()
        .gap(space::XS)
        .child(mode_tab(
            ColorBalanceView::ThreeWay,
            "3-Way",
            three_way_modified,
        ))
        .child(mode_tab(
            ColorBalanceView::Master,
            "Master",
            master_modified,
        ));

    let content = match s.color_balance_view {
        ColorBalanceView::ThreeWay => row()
            .w_full()
            .justify_between()
            .items_start()
            .child(single_color_wheel(s, k, WheelSlot::Shadows))
            .child(single_color_wheel(s, k, WheelSlot::Midtones))
            .child(single_color_wheel(s, k, WheelSlot::Highlights)),
        ColorBalanceView::Master => master_color_wheel_view(s, k),
    };

    let body = col()
        .w_full()
        .gap(space::GROUP_GAP)
        .child(mode_switcher)
        .child(content);

    let reset = mini_reset_btn(k, "Reset all color wheels", all_neutral, |s, tx| {
        s.update_slider_tx(true, tx, |r| {
            r.master_wheel = ColorWheel::default();
            r.shadows_wheel = ColorWheel::default();
            r.midtones_wheel = ColorWheel::default();
            r.highlights_wheel = ColorWheel::default();
        });
    });

    collapsible_section_card(
        k,
        "sec-color-wheels",
        "COLOR BALANCE",
        s.color_balance_open,
        !all_neutral,
        |s| s.color_balance_open = !s.color_balance_open,
        s.hovered_info,
        InfoTip {
            title: "3-Way & Master Color Balance",
            shortcut: None,
            description: "Applies independent hue/chroma tints and luminance offsets across Shadows, Midtones, Highlights, or the Master wheel. Click a wheel title to reset.",
            cli_key: "{shadows,midtones,highlights,master}_wheel  ·  MCP: rune_edit_photo",
            tier: "Tier 0 · Color",
        },
        Some(reset),
        body,
    )
}

/// Live HSV swatch dot for B&W Split Tone Shadow/Highlight hue capsules.
fn split_tone_hue_dot(hue_deg: f32, sat: f32) -> Color {
    let s = (0.35 + 0.65 * sat.clamp(0.0, 1.0)).clamp(0.35, 1.0);
    let (r, g, b) = hsv_to_rgb(hue_deg.rem_euclid(360.0) / 360.0, s, 0.92);
    tokens::srgb(r, g, b)
}

/// Collapsible 6-Channel Black & White Color Sensitivity Mixer + Capture One Split Tones.
fn bw_section(s: &AppState, k: &Kit) -> El {
    type BwChannelSpec = (
        &'static str,
        &'static str,
        Color,
        f32,
        fn(&mut DevelopRecipe, f32),
    );
    let bw = s.recipe.bw;
    let channels: [BwChannelSpec; 6] = [
        ("sl-bw-red", "Red", color::channel_red(), bw.red, |r, v| {
            r.bw.red = v;
        }),
        (
            "sl-bw-yellow",
            "Yellow",
            color::channel_yellow(),
            bw.yellow,
            |r, v| r.bw.yellow = v,
        ),
        (
            "sl-bw-green",
            "Green",
            color::channel_green(),
            bw.green,
            |r, v| r.bw.green = v,
        ),
        (
            "sl-bw-cyan",
            "Cyan",
            color::channel_cyan(),
            bw.cyan,
            |r, v| r.bw.cyan = v,
        ),
        (
            "sl-bw-blue",
            "Blue",
            color::channel_blue(),
            bw.blue,
            |r, v| r.bw.blue = v,
        ),
        (
            "sl-bw-magenta",
            "Magenta",
            color::channel_magenta(),
            bw.magenta,
            |r, v| r.bw.magenta = v,
        ),
    ];

    let mut body = col().w_full().gap(space::ROW_GAP);
    for (key, label, dot, val, setter) in channels {
        body = body.child(develop_slider_inner(
            k,
            key,
            label,
            Some(dot),
            val,
            -1.0..=1.0,
            format!("{:+.0}", val * 100.0),
            0.0,
            true,
            setter,
        ));
    }

    if bw.enabled {
        let split_mod =
            usize::from(bw.split_shadow_sat > 0.005) + usize::from(bw.split_highlight_sat > 0.005);
        body = body.child(advanced_disclosure_row(
            k,
            "adv-bw-split",
            "Split Tones",
            s.bw_split_open,
            split_mod,
            4,
            |s| s.bw_split_open = !s.bw_split_open,
        ));
        if s.bw_split_open {
            let shadow_dot = split_tone_hue_dot(bw.split_shadow_hue, bw.split_shadow_sat);
            let highlight_dot = split_tone_hue_dot(bw.split_highlight_hue, bw.split_highlight_sat);
            body = body
                .child(develop_slider_inner(
                    k,
                    "sl-bw-split-sh-hue",
                    "Shadow Hue",
                    Some(shadow_dot),
                    bw.split_shadow_hue,
                    0.0..=360.0,
                    format!("{:.0}°", bw.split_shadow_hue),
                    0.0,
                    true,
                    |r, v| r.bw.split_shadow_hue = v.clamp(0.0, 360.0),
                ))
                .child(develop_slider_inner(
                    k,
                    "sl-bw-split-sh-sat",
                    "Shadow Sat",
                    None,
                    bw.split_shadow_sat,
                    0.0..=1.0,
                    format!("{:.0}", bw.split_shadow_sat * 100.0),
                    0.0,
                    true,
                    |r, v| r.bw.split_shadow_sat = v.clamp(0.0, 1.0),
                ))
                .child(develop_slider_inner(
                    k,
                    "sl-bw-split-hi-hue",
                    "Highlight Hue",
                    Some(highlight_dot),
                    bw.split_highlight_hue,
                    0.0..=360.0,
                    format!("{:.0}°", bw.split_highlight_hue),
                    0.0,
                    true,
                    |r, v| r.bw.split_highlight_hue = v.clamp(0.0, 360.0),
                ))
                .child(develop_slider_inner(
                    k,
                    "sl-bw-split-hi-sat",
                    "Highlight Sat",
                    None,
                    bw.split_highlight_sat,
                    0.0..=1.0,
                    format!("{:.0}", bw.split_highlight_sat * 100.0),
                    0.0,
                    true,
                    |r, v| r.bw.split_highlight_sat = v.clamp(0.0, 1.0),
                ));
        }
    }

    let enable_sw = Switch::new(bw.enabled)
        .label("Enable Black & White")
        .hide_label()
        .on_toggle_tx(|s: &mut AppState, tx| {
            let next_on = !s.recipe.bw.enabled;
            if next_on {
                s.bw_open = true;
            }
            s.update_slider_tx(true, tx, |rec| rec.bw.enabled = next_on);
        })
        .build(k);

    let reset = mini_reset_btn(k, "Reset Black & White", bw.is_neutral(), |s, tx| {
        s.update_slider_tx(true, tx, |rec| rec.bw = BlackAndWhiteParams::default());
    });

    let actions = row()
        .items_center()
        .gap(space::SM)
        .child(enable_sw)
        .child(reset);

    collapsible_section_card(
        k,
        "sec-bw",
        "BLACK & WHITE",
        s.bw_open,
        !bw.is_neutral(),
        |s| s.bw_open = !s.bw_open,
        s.hovered_info,
        InfoTip {
            title: "6-Channel Black & White",
            shortcut: None,
            description: "Simulates panchromatic monochrome emulsion with 6 color filter channels plus optional silver/selenium Split Tones.",
            cli_key: "bw.{enabled,red,yellow,green,cyan,blue,magenta,split_*}",
            tier: "Tier 1 · Monochrome",
        },
        Some(actions),
        body,
    )
}

/// Live swatch color from HSV degrees/sat/value for Advanced Color Slices & Skin Tone.
fn target_swatch_color(hue_deg: f32, sat: f32, val: f32) -> Color {
    let (r, g, b) = hsv_to_rgb(
        hue_deg.rem_euclid(360.0) / 360.0,
        sat.clamp(0.25, 1.0),
        val.clamp(0.32, 0.96),
    );
    tokens::srgb(r, g, b)
}

/// Basic 8-band HSL Color Editor body (tab 1 of Capture One's 3-tab Color Editor).
fn color_editor_basic_body(s: &AppState, k: &Kit) -> El {
    let active_idx = s.selected_hsl_band.min(HSL_BAND_COUNT - 1);
    let band = s.recipe.hsl[active_idx];
    let band_name = HSL_BAND_NAMES[active_idx];

    let mut swatch_row = row()
        .key("hsl-swatches")
        .role("radiogroup")
        .label("Color band")
        .w_full()
        .justify_between()
        .items_center();

    for i in 0..HSL_BAND_COUNT {
        let selected = i == active_idx;
        let modified = !s.recipe.hsl[i].is_neutral();
        let swatch_col = color::hsl_band_swatch(i);

        let btn = col()
            .key(format!("hsl-swatch-{i}"))
            .role("radio")
            .label(HSL_BAND_NAMES[i])
            .checked(selected)
            .help(format!(
                "Edit {} hue, saturation and luminance",
                HSL_BAND_NAMES[i]
            ))
            .focusable(true)
            .focus_ring(
                border::FOCUS_RING,
                border::FOCUS_OFFSET_TIGHT,
                k.c.border_focus,
            )
            .cursor(Cursor::Pointer)
            .items_center()
            .gap(space::TINY)
            .child(
                row()
                    .size(size::SWATCH_RING, size::SWATCH_RING)
                    .center()
                    .radius(radius::circle(size::SWATCH_RING))
                    .fill(if selected {
                        k.c.layer_03
                    } else {
                        Color::TRANSPARENT
                    })
                    .hover(k.c.hover)
                    .border(
                        if selected {
                            border::ACTIVE
                        } else {
                            border::HAIRLINE
                        },
                        if selected {
                            k.c.border_focus
                        } else {
                            Color::TRANSPARENT
                        },
                    )
                    .child(
                        el().size(size::SWATCH_DOT, size::SWATCH_DOT)
                            .radius(radius::circle(size::SWATCH_DOT))
                            .fill(swatch_col)
                            .border(border::HAIRLINE, color::swatch_rim(k))
                            .a11y_hidden(),
                    ),
            )
            .child(
                el().size(size::MODIFIED_DOT, size::MODIFIED_DOT)
                    .radius(radius::circle(size::MODIFIED_DOT))
                    .fill(if modified {
                        k.c.text_accent
                    } else {
                        Color::TRANSPARENT
                    })
                    .a11y_hidden(),
            )
            .on_press(move |s: &mut AppState| {
                s.selected_hsl_band = i;
            });

        swatch_row = swatch_row.child(btn);
    }

    let band_header = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(
            row()
                .items_center()
                .gap(space::SM)
                .child(
                    el().size(size::BAND_DOT, size::BAND_DOT)
                        .radius(radius::circle(size::BAND_DOT))
                        .fill(color::hsl_band_swatch(active_idx))
                        .a11y_hidden(),
                )
                .child(
                    text(format!("{band_name} Band"))
                        .font(ty::control_label_medium(k))
                        .color(k.c.text_base),
                ),
        )
        .child(mini_reset_btn(
            k,
            "Reset color band",
            band.is_neutral(),
            move |s, tx| {
                s.update_slider_tx(true, tx, |r| {
                    r.hsl[active_idx] = rune_editor_core::HslBand::default();
                });
            },
        ));

    let hue_slider = develop_slider_inner(
        k,
        format!("sl-hsl-hue-{active_idx}"),
        "Hue",
        None,
        band.hue,
        -1.0..=1.0,
        format!("{:+.0}°", band.hue * 30.0),
        0.0,
        true,
        move |r, v| r.hsl[active_idx].hue = v,
    );
    let sat_slider = develop_slider_inner(
        k,
        format!("sl-hsl-sat-{active_idx}"),
        "Saturation",
        None,
        band.saturation,
        -1.0..=1.0,
        format!("{:+.0}", band.saturation * 100.0),
        0.0,
        true,
        move |r, v| r.hsl[active_idx].saturation = v,
    );
    let lum_slider = develop_slider_inner(
        k,
        format!("sl-hsl-lum-{active_idx}"),
        "Luminance",
        None,
        band.luminance,
        -1.0..=1.0,
        format!("{:+.0}", band.luminance * 100.0),
        0.0,
        true,
        move |r, v| r.hsl[active_idx].luminance = v,
    );

    col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(swatch_row)
        .child(band_header)
        .child(hue_slider)
        .child(sat_slider)
        .child(lum_slider)
}

/// Advanced Color Editor body (tab 2: 4 custom slices, eyedropper picker, View Range,
/// Uniformity, and Hue/Sat/Lightness adjustments).
fn color_editor_advanced_body(s: &AppState, k: &Kit) -> El {
    let idx = s.selected_color_slice.min(ADVANCED_COLOR_SLICE_COUNT - 1);
    let slice = s.recipe.advanced_color[idx].sanitized();
    let viewing_range = s.camera.view_color_range == idx as i32;

    let mut slice_row = row()
        .key("adv-color-slices")
        .role("radiogroup")
        .label("Advanced color slice")
        .w_full()
        .gap(space::XS);
    for i in 0..ADVANCED_COLOR_SLICE_COUNT {
        let sl = s.recipe.advanced_color[i].sanitized();
        let selected = i == idx;
        let dot_col = target_swatch_color(sl.target_hue, sl.target_sat, 0.90);
        let mut pill = row()
            .key(format!("adv-slice-{i}"))
            .role("radio")
            .label(format!("Color slice {}", i + 1))
            .checked(selected)
            .focusable(true)
            .focus_ring(
                border::FOCUS_RING,
                border::FOCUS_OFFSET_TIGHT,
                k.c.border_focus,
            )
            .cursor(Cursor::Pointer)
            .grow(1.0)
            .min_w(0.0)
            .h(size::PILL_H)
            .center()
            .gap(space::TIGHT)
            .radius(k.radius.sm)
            .fill(color::pill_fill(k, selected))
            .hover(k.c.hover)
            .border(
                if selected {
                    border::REGULAR
                } else {
                    border::HAIRLINE
                },
                if selected {
                    k.c.border_strong
                } else {
                    k.c.border_muted
                },
            )
            .child(
                el().size(size::CHANNEL_DOT, size::CHANNEL_DOT)
                    .radius(radius::circle(size::CHANNEL_DOT))
                    .fill(dot_col)
                    .border(border::HAIRLINE, color::swatch_rim(k))
                    .a11y_hidden(),
            )
            .child(
                text(format!("{}", i + 1))
                    .font(ty::section_header(k))
                    .color(if selected {
                        k.c.text_base
                    } else {
                        k.c.text_muted
                    }),
            );
        if sl.enabled {
            pill = pill.child(
                el().pin(Some(space::XL), None, Some(space::XL), Some(space::MICRO))
                    .h(space::XXS)
                    .radius(space::MICRO)
                    .fill(color::levels_modified_bar())
                    .a11y_hidden(),
            );
        }
        slice_row = slice_row.child(pill.on_press(move |s: &mut AppState| {
            s.set_selected_color_slice(i);
        }));
    }

    let swatch_col = target_swatch_color(slice.target_hue, slice.target_sat, 0.90);
    let pick_btn = IconButton::new(&icons::SELECT_ELEMENT)
        .variant(if s.color_slice_picker_active {
            IconButtonVariant::Contrast
        } else {
            IconButtonVariant::GhostMuted
        })
        .control_size(ControlSize::Mini)
        .label("Pick Color Slice Eyedropper")
        .help("Click any color on the photo to target this slice")
        .on_press_tx(|s: &mut AppState, tx| s.toggle_color_slice_picker_tx(tx))
        .build(k);
    let enable_sw = Switch::new(slice.enabled)
        .label(format!("Enable Slice {}", idx + 1))
        .hide_label()
        .on_toggle_tx(move |s: &mut AppState, tx| {
            let next_on = !s.recipe.advanced_color[idx].enabled;
            s.update_slider_tx(true, tx, |r| r.advanced_color[idx].enabled = next_on);
        })
        .build(k);
    let reset_slice = mini_reset_btn(
        k,
        "Reset active color slice",
        slice.is_neutral(),
        move |s, tx| {
            s.update_slider_tx(true, tx, |r| {
                r.advanced_color[idx] = ColorEditorSlice::default();
            });
        },
    );

    let slice_header = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(
            row()
                .items_center()
                .gap(space::SM)
                .child(
                    el().size(size::TARGET_SWATCH, size::TARGET_SWATCH)
                        .radius(k.radius.xs)
                        .fill(swatch_col)
                        .border(border::HAIRLINE, color::swatch_rim(k))
                        .a11y_hidden(),
                )
                .child(
                    text(format!("Slice {} · {:.0}°", idx + 1, slice.target_hue))
                        .font(ty::mono_compact(k))
                        .color(k.c.text_base),
                ),
        )
        .child(
            row()
                .items_center()
                .gap(space::XS)
                .child(pick_btn)
                .child(enable_sw)
                .child(reset_slice),
        );

    let view_range_row = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(
            text("View Selected Color Range")
                .font(ty::control_label(k))
                .color(if viewing_range {
                    color::color_range_hud()
                } else {
                    k.c.text_muted
                }),
        )
        .child(
            Switch::new(viewing_range)
                .label("View Selected Color Range")
                .hide_label()
                .on_toggle(move |s: &mut AppState| {
                    s.toggle_view_color_range(idx as i32);
                })
                .build(k),
        );

    let range_mod = usize::from(
        (slice.hue_range - descriptor_default("advanced_color.0.hue_range")).abs() > 0.5,
    ) + usize::from(
        (slice.sat_range - descriptor_default("advanced_color.0.sat_range")).abs() > 0.01,
    ) + usize::from(
        (slice.smoothness - descriptor_default("advanced_color.0.smoothness")).abs() > 0.01,
    ) + usize::from(slice.uniformity.abs() > 0.005);

    let mut body = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(slice_row)
        .child(slice_header)
        .child(view_range_row)
        .child(develop_slider_inner(
            k,
            format!("sl-adv-{idx}-hue"),
            "Hue",
            None,
            slice.hue_shift,
            descriptor_range("advanced_color.0.hue_shift"),
            format!("{:+.0}°", slice.hue_shift * 60.0),
            descriptor_default("advanced_color.0.hue_shift"),
            true,
            move |r, v| r.advanced_color[idx].hue_shift = v,
        ))
        .child(develop_slider_inner(
            k,
            format!("sl-adv-{idx}-sat"),
            "Saturation",
            None,
            slice.saturation,
            descriptor_range("advanced_color.0.saturation"),
            format!("{:+.0}", slice.saturation * 100.0),
            descriptor_default("advanced_color.0.saturation"),
            true,
            move |r, v| r.advanced_color[idx].saturation = v,
        ))
        .child(develop_slider_inner(
            k,
            format!("sl-adv-{idx}-lum"),
            "Lightness",
            None,
            slice.lightness,
            descriptor_range("advanced_color.0.lightness"),
            format!("{:+.0}", slice.lightness * 100.0),
            descriptor_default("advanced_color.0.lightness"),
            true,
            move |r, v| r.advanced_color[idx].lightness = v,
        ))
        .child(advanced_disclosure_row(
            k,
            "adv-color-range-row",
            "Range & Uniformity",
            s.color_adv_range_open,
            range_mod,
            4,
            |s| s.color_adv_range_open = !s.color_adv_range_open,
        ));

    if s.color_adv_range_open {
        body = body
            .child(develop_slider_inner(
                k,
                format!("sl-adv-{idx}-hue-range"),
                "Hue Range",
                None,
                slice.hue_range,
                descriptor_range("advanced_color.0.hue_range"),
                format!("{:.0}°", slice.hue_range),
                descriptor_default("advanced_color.0.hue_range"),
                true,
                move |r, v| r.advanced_color[idx].hue_range = v,
            ))
            .child(develop_slider_inner(
                k,
                format!("sl-adv-{idx}-sat-range"),
                "Sat Range",
                None,
                slice.sat_range,
                descriptor_range("advanced_color.0.sat_range"),
                format!("{:.0}%", slice.sat_range * 100.0),
                descriptor_default("advanced_color.0.sat_range"),
                true,
                move |r, v| r.advanced_color[idx].sat_range = v,
            ))
            .child(develop_slider_inner(
                k,
                format!("sl-adv-{idx}-smooth"),
                "Smoothness",
                None,
                slice.smoothness,
                descriptor_range("advanced_color.0.smoothness"),
                format!("{:.0}", slice.smoothness * 100.0),
                descriptor_default("advanced_color.0.smoothness"),
                true,
                move |r, v| r.advanced_color[idx].smoothness = v,
            ))
            .child(develop_slider_inner(
                k,
                format!("sl-adv-{idx}-uniformity"),
                "Uniformity",
                None,
                slice.uniformity,
                descriptor_range("advanced_color.0.uniformity"),
                format!("{:.0}", slice.uniformity * 100.0),
                descriptor_default("advanced_color.0.uniformity"),
                true,
                move |r, v| r.advanced_color[idx].uniformity = v,
            ));
    }
    body
}

/// Skin Tone Uniformity body (tab 3 of Capture One's Color Editor).
fn color_editor_skin_tone_body(s: &AppState, k: &Kit) -> El {
    let st = s.recipe.skin_tone.sanitized();
    let viewing_range = s.camera.view_color_range == 4;
    let swatch_col = target_swatch_color(st.target_hue, st.target_sat, st.target_lightness);

    let pick_btn = IconButton::new(&icons::SELECT_ELEMENT)
        .variant(if s.skin_picker_active {
            IconButtonVariant::Contrast
        } else {
            IconButtonVariant::GhostMuted
        })
        .control_size(ControlSize::Mini)
        .label("Pick Skin Tone Eyedropper")
        .help("Click skin on the photo to sample target hue, saturation and lightness")
        .on_press_tx(|s: &mut AppState, tx| s.toggle_skin_picker_tx(tx))
        .build(k);
    let enable_sw = Switch::new(st.enabled)
        .label("Enable Skin Tone Uniformity")
        .hide_label()
        .on_toggle_tx(|s: &mut AppState, tx| {
            let next_on = !s.recipe.skin_tone.enabled;
            s.update_slider_tx(true, tx, |r| r.skin_tone.enabled = next_on);
        })
        .build(k);

    let target_header = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(
            row()
                .items_center()
                .gap(space::SM)
                .child(
                    el().size(size::TARGET_SWATCH, size::TARGET_SWATCH)
                        .radius(k.radius.xs)
                        .fill(swatch_col)
                        .border(border::HAIRLINE, color::swatch_rim(k))
                        .a11y_hidden(),
                )
                .child(
                    text(format!(
                        "{:.0}° · S{:.0} L{:.0}",
                        st.target_hue,
                        st.target_sat * 100.0,
                        st.target_lightness * 100.0
                    ))
                    .font(ty::mono_compact(k))
                    .color(k.c.text_base),
                ),
        )
        .child(
            row()
                .items_center()
                .gap(space::XS)
                .child(pick_btn)
                .child(enable_sw),
        );

    let view_range_row = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(
            text("View Selected Color Range")
                .font(ty::control_label(k))
                .color(if viewing_range {
                    color::color_range_hud()
                } else {
                    k.c.text_muted
                }),
        )
        .child(
            Switch::new(viewing_range)
                .label("View Selected Skin Tone Range")
                .hide_label()
                .on_toggle(|s: &mut AppState| {
                    s.toggle_view_color_range(4);
                })
                .build(k),
        );

    let skin_adv_mod = usize::from(st.amount_hue.abs() > 0.005)
        + usize::from(st.amount_sat.abs() > 0.005)
        + usize::from(st.amount_lightness.abs() > 0.005)
        + usize::from((st.smoothness - descriptor_default("skin_tone.smoothness")).abs() > 0.01);

    let mut body = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(target_header)
        .child(view_range_row)
        .child(
            text("UNIFORMITY")
                .font(ty::section_header(k))
                .color(k.c.text_muted),
        )
        .child(develop_slider_inner(
            k,
            "sl-skin-uni-hue",
            "Hue",
            None,
            st.uniformity_hue,
            descriptor_range("skin_tone.uniformity_hue"),
            format!("{:.0}", st.uniformity_hue * 100.0),
            descriptor_default("skin_tone.uniformity_hue"),
            true,
            |r, v| r.skin_tone.uniformity_hue = v,
        ))
        .child(develop_slider_inner(
            k,
            "sl-skin-uni-sat",
            "Saturation",
            None,
            st.uniformity_sat,
            descriptor_range("skin_tone.uniformity_sat"),
            format!("{:.0}", st.uniformity_sat * 100.0),
            descriptor_default("skin_tone.uniformity_sat"),
            true,
            |r, v| r.skin_tone.uniformity_sat = v,
        ))
        .child(develop_slider_inner(
            k,
            "sl-skin-uni-lum",
            "Lightness",
            None,
            st.uniformity_lightness,
            descriptor_range("skin_tone.uniformity_lightness"),
            format!("{:.0}", st.uniformity_lightness * 100.0),
            descriptor_default("skin_tone.uniformity_lightness"),
            true,
            |r, v| r.skin_tone.uniformity_lightness = v,
        ))
        .child(advanced_disclosure_row(
            k,
            "adv-skin-amount-row",
            "Amount & Smoothness",
            s.skin_tone_advanced_open,
            skin_adv_mod,
            4,
            |s| s.skin_tone_advanced_open = !s.skin_tone_advanced_open,
        ));

    if s.skin_tone_advanced_open {
        body = body
            .child(develop_slider_inner(
                k,
                "sl-skin-amt-hue",
                "Hue",
                None,
                st.amount_hue,
                descriptor_range("skin_tone.amount_hue"),
                format!("{:+.0}°", st.amount_hue * 35.0),
                descriptor_default("skin_tone.amount_hue"),
                true,
                |r, v| r.skin_tone.amount_hue = v,
            ))
            .child(develop_slider_inner(
                k,
                "sl-skin-amt-sat",
                "Saturation",
                None,
                st.amount_sat,
                descriptor_range("skin_tone.amount_sat"),
                format!("{:+.0}", st.amount_sat * 100.0),
                descriptor_default("skin_tone.amount_sat"),
                true,
                |r, v| r.skin_tone.amount_sat = v,
            ))
            .child(develop_slider_inner(
                k,
                "sl-skin-amt-lum",
                "Lightness",
                None,
                st.amount_lightness,
                descriptor_range("skin_tone.amount_lightness"),
                format!("{:+.0}", st.amount_lightness * 100.0),
                descriptor_default("skin_tone.amount_lightness"),
                true,
                |r, v| r.skin_tone.amount_lightness = v,
            ))
            .child(develop_slider_inner(
                k,
                "sl-skin-smooth",
                "Smoothness",
                None,
                st.smoothness,
                descriptor_range("skin_tone.smoothness"),
                format!("{:.0}", st.smoothness * 100.0),
                descriptor_default("skin_tone.smoothness"),
                true,
                |r, v| r.skin_tone.smoothness = v,
            ));
    }
    body
}

/// Capture One Pro 3-tab Color Editor (`Basic | Advanced | Skin Tone`).
fn hsl_editor_section(s: &AppState, k: &Kit) -> El {
    let basic_modified = s.recipe.hsl.iter().any(|b| !b.is_neutral());
    let adv_modified = s.recipe.advanced_color.iter().any(|sl| !sl.is_neutral());
    let skin_modified = !s.recipe.skin_tone.is_neutral();
    let any_modified = basic_modified || adv_modified || skin_modified;

    let mut mode_row = row()
        .key("color-editor-modes")
        .role("radiogroup")
        .label("Color editor mode")
        .w_full()
        .gap(space::XS);
    for mode in ColorEditorMode::ALL {
        let active = s.color_editor_mode == mode;
        let mode_mod = match mode {
            ColorEditorMode::Basic => basic_modified,
            ColorEditorMode::Advanced => adv_modified,
            ColorEditorMode::SkinTone => skin_modified,
        };
        let mut pill = row()
            .key(format!("ce-mode-{}", mode.label()))
            .role("radio")
            .label(format!("{} color editor", mode.label()))
            .checked(active)
            .focusable(true)
            .focus_ring(
                border::FOCUS_RING,
                border::FOCUS_OFFSET_TIGHT,
                k.c.border_focus,
            )
            .cursor(Cursor::Pointer)
            .grow(1.0)
            .min_w(0.0)
            .h(size::PILL_H)
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
                if active {
                    k.c.border_strong
                } else {
                    k.c.border_muted
                },
            )
            .child(
                text(mode.label())
                    .font(ty::section_header(k))
                    .color(if active {
                        k.c.text_base
                    } else {
                        k.c.text_muted
                    }),
            );
        if mode_mod {
            pill = pill.child(
                el().pin(Some(space::XL), None, Some(space::XL), Some(space::MICRO))
                    .h(space::XXS)
                    .radius(space::MICRO)
                    .fill(color::levels_modified_bar())
                    .a11y_hidden(),
            );
        }
        mode_row = mode_row.child(pill.on_press(move |s: &mut AppState| {
            s.set_color_editor_mode(mode);
        }));
    }

    let mode_body = match s.color_editor_mode {
        ColorEditorMode::Basic => color_editor_basic_body(s, k),
        ColorEditorMode::Advanced => color_editor_advanced_body(s, k),
        ColorEditorMode::SkinTone => color_editor_skin_tone_body(s, k),
    };

    let (reset_label, reset_disabled) = match s.color_editor_mode {
        ColorEditorMode::Basic => ("Reset Basic HSL bands", !basic_modified),
        ColorEditorMode::Advanced => ("Reset all Advanced Color slices", !adv_modified),
        ColorEditorMode::SkinTone => ("Reset Skin Tone Uniformity", !skin_modified),
    };
    let active_mode = s.color_editor_mode;
    let reset_btn = mini_reset_btn(k, reset_label, reset_disabled, move |s, tx| {
        s.update_slider_tx(true, tx, |r| match active_mode {
            ColorEditorMode::Basic => {
                r.hsl = [rune_editor_core::HslBand::default(); HSL_BAND_COUNT];
            }
            ColorEditorMode::Advanced => {
                r.advanced_color = [ColorEditorSlice::default(); ADVANCED_COLOR_SLICE_COUNT];
            }
            ColorEditorMode::SkinTone => {
                r.skin_tone = SkinToneParams::default();
            }
        });
    });

    let body = col()
        .w_full()
        .gap(space::CARD_HEADER_GAP)
        .child(mode_row)
        .child(mode_body);

    collapsible_section_card(
        k,
        "sec-hsl",
        "COLOR EDITOR",
        s.hsl_open,
        any_modified,
        |s| {
            s.hsl_open = !s.hsl_open;
            if !s.hsl_open {
                s.camera.view_color_range = -1;
            }
        },
        s.hovered_info,
        InfoTip {
            title: "3-Tab Color Editor",
            shortcut: None,
            description: "Targeted 8-band Basic HSL, 4-slice Advanced color targeting with View Range mask, and Capture One Skin Tone Uniformity.",
            cli_key: "hsl.*  ·  advanced_color.*  ·  skin_tone.*",
            tier: "Tier 1 · Selective Color",
        },
        Some(reset_btn),
        body,
    )
}

/// Capture One-style Base Characteristics card exposing Processing Mode (`Photography | Film Negative`)
/// and the 4 Film Curves.
fn base_characteristics_section(s: &AppState, k: &Kit) -> El {
    let active_curve = s.recipe.film_curve;
    let active_mode = s.recipe.processing_mode;
    let is_default =
        active_curve == FilmCurve::FilmStandard && active_mode == ProcessingMode::Photography;

    let mut mode_row = row()
        .key("processing-mode-row")
        .role("radiogroup")
        .label("Processing mode")
        .w_full()
        .gap(space::PILL_GAP);
    for mode in ProcessingMode::ALL {
        let active = active_mode == mode;
        mode_row = mode_row.child(crop_pill_btn(
            k,
            format!("proc-mode-{}", mode.cli_slug()),
            "radio",
            format!("Processing mode {}", mode.label()),
            mode.label(),
            size::PILL_H,
            active,
            k.c.text_muted,
            move |s, tx| s.set_processing_mode_tx(mode, tx),
        ));
    }

    let mut grid = col()
        .key("film-curve-grid")
        .role("radiogroup")
        .label("Base characteristics film curve")
        .w_full()
        .gap(space::PILL_GAP);

    for chunk in FilmCurve::ALL.chunks(2) {
        let mut r = row().w_full().gap(space::PILL_GAP);
        for &curve in chunk {
            let active = active_curve == curve;
            r = r.child(crop_pill_btn(
                k,
                format!("film-curve-{}", curve.cli_slug()),
                "radio",
                format!("Film curve {}", curve.label()),
                curve.label(),
                size::PILL_H,
                active,
                k.c.text_muted,
                move |s, tx| s.set_film_curve_tx(curve, tx),
            ));
        }
        grid = grid.child(r);
    }

    let body = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(
            row()
                .w_full()
                .justify_between()
                .items_center()
                .child(
                    text("Mode")
                        .font(ty::control_label(k))
                        .color(k.c.text_muted),
                )
                .child(text(active_mode.label()).font(ty::mono_compact(k)).color(
                    if active_mode == ProcessingMode::FilmNegative {
                        color::film_negative_accent()
                    } else {
                        k.c.text_faint
                    },
                )),
        )
        .child(mode_row)
        .child(
            row()
                .w_full()
                .justify_between()
                .items_center()
                .child(
                    text("Film Curve")
                        .font(ty::control_label(k))
                        .color(k.c.text_muted),
                )
                .child(text(active_curve.label()).font(ty::mono_compact(k)).color(
                    if active_curve == FilmCurve::FilmStandard {
                        k.c.text_faint
                    } else {
                        k.c.text_base
                    },
                )),
        )
        .child(grid);

    let reset = mini_reset_btn(k, "Reset Base Characteristics", is_default, |s, tx| {
        s.update_slider_tx(true, tx, |rec| {
            rec.film_curve = FilmCurve::FilmStandard;
            rec.processing_mode = ProcessingMode::Photography;
        });
    });

    collapsible_section_card(
        k,
        "sec-base-char",
        "BASE CHARACTERISTICS",
        s.base_open,
        !is_default,
        |s| s.base_open = !s.base_open,
        s.hovered_info,
        InfoTip {
            title: "Base Characteristics",
            shortcut: None,
            description: "Selects scene-referred Photography vs. C-41 Film Negative pipeline and the base film response curve (Standard, High Contrast, Extra Shadow, Linear).",
            cli_key: "processing_mode  ·  film_curve",
            tier: "Tier 0 · Pipeline",
        },
        Some(reset),
        body,
    )
}

/// Phase 5 Capture One Film Negative Conversion card (revealed when `Film Negative` mode is active).
fn film_negative_section(s: &AppState, k: &Kit) -> El {
    let fn_params = s.recipe.film_negative.sanitized();
    let is_default = fn_params.is_default();

    let swatch_col = tokens::srgb(
        linear_to_srgb((fn_params.base_r / 2.0).clamp(0.0, 1.0)),
        linear_to_srgb((fn_params.base_g / 2.0).clamp(0.0, 1.0)),
        linear_to_srgb((fn_params.base_b / 2.0).clamp(0.0, 1.0)),
    );

    let base_row = row()
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
                    el().size(size::TARGET_SWATCH, size::TARGET_SWATCH)
                        .radius(k.radius.xs)
                        .fill(swatch_col)
                        .border(border::HAIRLINE, k.c.border_strong)
                        .a11y_hidden(),
                )
                .child(
                    text("Base Mask")
                        .font(ty::control_label(k))
                        .color(k.c.text_muted),
                ),
        )
        .child(
            text(format!(
                "R {:.2}  G {:.2}  B {:.2}",
                fn_params.base_r, fn_params.base_g, fn_params.base_b
            ))
            .font(ty::mono_micro(k))
            .color(k.c.text_base),
        );

    let action_row = row()
        .w_full()
        .gap(space::XS)
        .child(crop_pill_btn(
            k,
            "film-neg-pick-base",
            "button",
            "Pick Film Base from raw film border on canvas",
            if s.film_base_picker_active {
                "Picking Base…"
            } else {
                "Pick Film Base"
            },
            size::PILL_H,
            s.film_base_picker_active,
            k.c.text_base,
            |s, tx| s.toggle_film_base_picker_tx(tx),
        ))
        .child(crop_pill_btn(
            k,
            "film-neg-auto-convert",
            "button",
            "Auto-sample border and convert film negative",
            "Auto Base",
            size::PILL_H,
            false,
            k.c.text_muted,
            |s, tx| s.convert_film_negative_auto_tx(tx),
        ));

    let def_fn = FilmNegativeParams::default();
    let fn_rgb_mod = usize::from((fn_params.base_r - def_fn.base_r).abs() > 0.01)
        + usize::from((fn_params.base_g - def_fn.base_g).abs() > 0.01)
        + usize::from((fn_params.base_b - def_fn.base_b).abs() > 0.01);

    let mut body = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(base_row)
        .child(action_row)
        .child(develop_slider_inner(
            k,
            "sl-fn-density",
            "Density",
            None,
            fn_params.density,
            descriptor_range("film_negative.density"),
            format!("{:+.2} EV", fn_params.density),
            descriptor_default("film_negative.density"),
            true,
            |rec, v| rec.film_negative.density = v,
        ))
        .child(develop_slider_inner(
            k,
            "sl-fn-contrast",
            "Contrast",
            None,
            fn_params.contrast,
            descriptor_range("film_negative.contrast"),
            format!("{:+.0}", fn_params.contrast * 100.0),
            descriptor_default("film_negative.contrast"),
            true,
            |rec, v| rec.film_negative.contrast = v,
        ))
        .child(develop_slider_inner(
            k,
            "sl-fn-black-clip",
            "Black Clip",
            None,
            fn_params.black_clip,
            descriptor_range("film_negative.black_clip"),
            format!("{:.0}%", fn_params.black_clip * 100.0),
            descriptor_default("film_negative.black_clip"),
            true,
            |rec, v| rec.film_negative.black_clip = v,
        ))
        .child(develop_slider_inner(
            k,
            "sl-fn-white-clip",
            "White Clip",
            None,
            fn_params.white_clip,
            descriptor_range("film_negative.white_clip"),
            format!("{:.0}%", fn_params.white_clip * 100.0),
            descriptor_default("film_negative.white_clip"),
            true,
            |rec, v| rec.film_negative.white_clip = v,
        ))
        .child(advanced_disclosure_row(
            k,
            "adv-film-neg-rgb",
            "Base Mask RGB",
            s.film_neg_advanced_open,
            fn_rgb_mod,
            3,
            |s| s.film_neg_advanced_open = !s.film_neg_advanced_open,
        ));

    if s.film_neg_advanced_open {
        body = body
            .child(develop_slider_inner(
                k,
                "sl-fn-base-r",
                "Base Red",
                Some(color::channel_red()),
                fn_params.base_r,
                descriptor_range("film_negative.base_r"),
                format!("{:.2}", fn_params.base_r),
                descriptor_default("film_negative.base_r"),
                true,
                |rec, v| rec.film_negative.base_r = v,
            ))
            .child(develop_slider_inner(
                k,
                "sl-fn-base-g",
                "Base Green",
                Some(color::channel_green()),
                fn_params.base_g,
                descriptor_range("film_negative.base_g"),
                format!("{:.2}", fn_params.base_g),
                descriptor_default("film_negative.base_g"),
                true,
                |rec, v| rec.film_negative.base_g = v,
            ))
            .child(develop_slider_inner(
                k,
                "sl-fn-base-b",
                "Base Blue",
                Some(color::channel_blue()),
                fn_params.base_b,
                descriptor_range("film_negative.base_b"),
                format!("{:.2}", fn_params.base_b),
                descriptor_default("film_negative.base_b"),
                true,
                |rec, v| rec.film_negative.base_b = v,
            ));
    }

    let reset = mini_reset_btn(k, "Reset Film Negative", is_default, |s, tx| {
        s.reset_film_negative_tx(tx);
    });

    collapsible_section_card(
        k,
        "sec-film-negative",
        "FILM NEGATIVE",
        s.film_neg_open,
        !is_default || fn_params != FilmNegativeParams::default(),
        |s| s.film_neg_open = !s.film_neg_open,
        s.hovered_info,
        InfoTip {
            title: "C-41 Film Negative Inversion",
            shortcut: None,
            description: "Neutralizes the orange C-41 film base mask in linear space and maps negative transmittance to positive density and contrast.",
            cli_key: "film_negative.{base_r,base_g,base_b,density,contrast,black_clip,white_clip}",
            tier: "Tier 0 · Analog",
        },
        Some(reset),
        body,
    )
}

fn light_tab_content(s: &AppState, k: &Kit) -> El {
    let r = &s.recipe;

    let wb_neutral =
        r.temperature.abs() < 0.005 && r.tint.abs() < 0.005 && r.wb_preset == WbPreset::AsShot;
    let effective_wb_preset = if r.wb_preset == WbPreset::AsShot
        && (r.temperature.abs() > 0.005 || r.tint.abs() > 0.005)
    {
        WbPreset::Custom
    } else {
        r.wb_preset
    };
    let kelvin = temperature_to_kelvin(r.temperature);

    // Compact White Balance Preset picker (inline disclosure keeps WB calm by default).
    let wb_presets_row1 = [
        WbPreset::AsShot,
        WbPreset::Daylight,
        WbPreset::Cloudy,
        WbPreset::Shade,
    ];
    let wb_presets_row2 = [WbPreset::Tungsten, WbPreset::Fluorescent, WbPreset::Flash];
    let build_wb_row = |presets: &[WbPreset]| {
        let mut row_el = row().w_full().gap(space::PILL_GAP);
        for &preset in presets {
            let active = effective_wb_preset == preset;
            row_el = row_el.child(crop_pill_btn(
                k,
                format!("wb-preset-{}", preset.cli_slug()),
                "radio",
                format!("White balance preset {}", preset.label()),
                preset.label(),
                size::PILL_COMPACT_H,
                active,
                k.c.text_muted,
                move |s, tx| s.apply_wb_preset_tx(preset, tx),
            ));
        }
        row_el
    };

    let wb_preset_toggle = row()
        .key("wb-preset-toggle")
        .role("button")
        .label("Toggle White Balance Illuminant Presets")
        .expanded(s.wb_presets_open)
        .cursor(Cursor::Pointer)
        .w_full()
        .h(size::PILL_H)
        .px(space::MD)
        .items_center()
        .justify_between()
        .radius(k.radius.md)
        .fill(if s.wb_presets_open {
            k.c.layer_02
        } else {
            k.c.bg_base
        })
        .hover(k.c.layer_02)
        .border(border::HAIRLINE, k.c.border_base)
        .child(
            text("Preset")
                .font(ty::control_label(k))
                .color(k.c.text_muted),
        )
        .child(
            row()
                .items_center()
                .gap(space::SM)
                .child(
                    text(effective_wb_preset.label())
                        .font(ty::mono_compact(k))
                        .color(if effective_wb_preset == WbPreset::AsShot {
                            k.c.text_faint
                        } else {
                            k.c.text_base
                        }),
                )
                .child(
                    icon(
                        if s.wb_presets_open {
                            &icons::CHEVRON_DOWN
                        } else {
                            &icons::CHEVRON_RIGHT
                        },
                        size::ICON_XS,
                    )
                    .color(k.c.icon_muted)
                    .a11y_hidden(),
                ),
        )
        .on_press(|s: &mut AppState| s.wb_presets_open = !s.wb_presets_open);

    let mut wb_preset_block = col()
        .key("wb-presets")
        .role("radiogroup")
        .label("White balance preset")
        .w_full()
        .gap(space::PILL_GAP)
        .child(wb_preset_toggle);

    if s.wb_presets_open {
        wb_preset_block = wb_preset_block
            .child(build_wb_row(&wb_presets_row1))
            .child(build_wb_row(&wb_presets_row2));
    }

    let wb_controls = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(wb_preset_block)
        .child(develop_scalar_slider(
            k,
            "sl-temp",
            "temperature",
            "Temperature",
            r.temperature,
            format!("{kelvin} K"),
            |rec, v| {
                rec.temperature = v;
                rec.wb_preset = WbPreset::Custom;
            },
        ))
        .child(develop_scalar_slider(
            k,
            "sl-tint",
            "tint",
            "Tint",
            r.tint,
            format!("{:+.0}", r.tint * 100.0),
            |rec, v| {
                rec.tint = v;
                rec.wb_preset = WbPreset::Custom;
            },
        ));
    let wb_picker = IconButton::new(&icons::SELECT_ELEMENT)
        .variant(if s.wb_eyedropper_active {
            IconButtonVariant::Contrast
        } else {
            IconButtonVariant::GhostMuted
        })
        .control_size(ControlSize::Mini)
        .label("Pick White Balance Eyedropper")
        .help("Click any neutral gray pixel on the photo to neutralize white balance")
        .on_press_tx(|s: &mut AppState, tx| s.toggle_wb_eyedropper_tx(tx))
        .build(k);
    let wb_actions = row()
        .items_center()
        .gap(space::XS)
        .child(wb_picker)
        .child(mini_auto_btn(
            k,
            "btn-auto-wb",
            "Auto White Balance",
            |s, tx| s.auto_white_balance_tx(tx),
        ))
        .child(mini_reset_btn(
            k,
            "Reset White Balance",
            wb_neutral,
            |s, tx| {
                s.update_slider_tx(true, tx, |rec| {
                    rec.temperature = scalar_default("temperature");
                    rec.tint = scalar_default("tint");
                    rec.wb_preset = WbPreset::AsShot;
                });
            },
        ));

    let tone_neutral = r.exposure.abs() < 0.005
        && r.contrast.abs() < 0.005
        && r.brightness.abs() < 0.005
        && r.highlights.abs() < 0.005
        && r.shadows.abs() < 0.005
        && r.whites.abs() < 0.005
        && r.blacks.abs() < 0.005;
    let tone_adv_mod = usize::from(r.brightness.abs() > 0.005)
        + usize::from(r.whites.abs() > 0.005)
        + usize::from(r.blacks.abs() > 0.005);

    let mut tone_controls = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(develop_scalar_slider(
            k,
            "sl-exposure",
            "exposure",
            "Exposure",
            r.exposure,
            format!("{:+.2} EV", r.exposure),
            |rec, v| rec.exposure = v,
        ))
        .child(develop_scalar_slider(
            k,
            "sl-contrast",
            "contrast",
            "Contrast",
            r.contrast,
            format!("{:+.0}", r.contrast * 100.0),
            |rec, v| rec.contrast = v,
        ))
        .child(develop_scalar_slider(
            k,
            "sl-highlights",
            "highlights",
            "Highlights",
            r.highlights,
            format!("{:+.0}", r.highlights * 100.0),
            |rec, v| rec.highlights = v,
        ))
        .child(develop_scalar_slider(
            k,
            "sl-shadows",
            "shadows",
            "Shadows",
            r.shadows,
            format!("{:+.0}", r.shadows * 100.0),
            |rec, v| rec.shadows = v,
        ))
        .child(advanced_disclosure_row(
            k,
            "adv-tone-fine",
            "Fine Range",
            s.tone_advanced_open,
            tone_adv_mod,
            3,
            |s| s.tone_advanced_open = !s.tone_advanced_open,
        ));

    if s.tone_advanced_open {
        tone_controls = tone_controls
            .child(develop_scalar_slider(
                k,
                "sl-brightness",
                "brightness",
                "Brightness",
                r.brightness,
                format!("{:+.0}", r.brightness * 100.0),
                |rec, v| rec.brightness = v,
            ))
            .child(develop_scalar_slider(
                k,
                "sl-whites",
                "whites",
                "Whites",
                r.whites,
                format!("{:+.0}", r.whites * 100.0),
                |rec, v| rec.whites = v,
            ))
            .child(develop_scalar_slider(
                k,
                "sl-blacks",
                "blacks",
                "Blacks",
                r.blacks,
                format!("{:+.0}", r.blacks * 100.0),
                |rec, v| rec.blacks = v,
            ));
    }

    let tone_actions = row()
        .items_center()
        .gap(space::XS)
        .child(mini_auto_btn(
            k,
            "btn-auto-tone",
            "Auto Exposure & Tone",
            |s, tx| s.auto_tone_tx(tx),
        ))
        .child(mini_reset_btn(
            k,
            "Reset Exposure & Tone",
            tone_neutral,
            |s, tx| {
                s.update_slider_tx(true, tx, |rec| {
                    rec.exposure = scalar_default("exposure");
                    rec.contrast = scalar_default("contrast");
                    rec.brightness = scalar_default("brightness");
                    rec.highlights = scalar_default("highlights");
                    rec.shadows = scalar_default("shadows");
                    rec.whites = scalar_default("whites");
                    rec.blacks = scalar_default("blacks");
                });
            },
        ));

    let mut stack = col()
        .w_full()
        .gap(space::CARD_GAP)
        .child(base_characteristics_section(s, k));
    if s.recipe.processing_mode == ProcessingMode::FilmNegative {
        stack = stack.child(film_negative_section(s, k));
    }
    stack
        .child(collapsible_section_card(
            k,
            "sec-wb",
            "WHITE BALANCE",
            s.wb_open,
            !wb_neutral,
            |s| s.wb_open = !s.wb_open,
            s.hovered_info,
            InfoTip {
                title: "White Balance",
                shortcut: Some("T / Y"),
                description: "Calibrates color temperature (Kelvin) and green/magenta tint in linear scene-referred space. Use the eyedropper on neutral gray or click A for Auto WB.",
                cli_key: "wb_preset  ·  temperature [-1..1]  ·  tint [-1..1]",
                tier: "Tier 0 · Light",
            },
            Some(wb_actions),
            wb_controls,
        ))
        .child(collapsible_section_card(
            k,
            "sec-tone",
            "EXPOSURE & TONE",
            s.tone_open,
            !tone_neutral,
            |s| s.tone_open = !s.tone_open,
            s.hovered_info,
            InfoTip {
                title: "Exposure & Tone",
                shortcut: Some("Q / W / A / S"),
                description: "Scene-referred EV exposure, midtone contrast, and highlight/shadow recovery. Expand Fine Range for Brightness, Whites, and Blacks.",
                cli_key: "exposure  ·  contrast  ·  highlights  ·  shadows  ·  brightness  ·  whites  ·  blacks",
                tier: "Tier 0 · Tone",
            },
            Some(tone_actions),
            tone_controls,
        ))
        .child(levels_section(s, k))
        .child(tone_curve_editor(s, k))
}

fn color_tab_content(s: &AppState, k: &Kit) -> El {
    let r = &s.recipe;
    let presence_neutral = r.vibrance.abs() < 0.005 && r.saturation.abs() < 0.005;
    let presence_controls = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(develop_scalar_slider(
            k,
            "sl-vibrance",
            "vibrance",
            "Vibrance",
            r.vibrance,
            format!("{:+.0}", r.vibrance * 100.0),
            |rec, v| rec.vibrance = v,
        ))
        .child(develop_scalar_slider(
            k,
            "sl-saturation",
            "saturation",
            "Saturation",
            r.saturation,
            format!("{:+.0}", r.saturation * 100.0),
            |rec, v| rec.saturation = v,
        ));
    let presence_reset = mini_reset_btn(
        k,
        "Reset Vibrance & Saturation",
        presence_neutral,
        |s, tx| {
            s.update_slider_tx(true, tx, |rec| {
                rec.vibrance = scalar_default("vibrance");
                rec.saturation = scalar_default("saturation");
            });
        },
    );

    col()
        .w_full()
        .gap(space::CARD_GAP)
        .child(color_wheels_section(s, k))
        .child(collapsible_section_card(
            k,
            "sec-presence",
            "PRESENCE",
            s.presence_open,
            !presence_neutral,
            |s| s.presence_open = !s.presence_open,
            s.hovered_info,
            InfoTip {
                title: "Presence (Vibrance & Saturation)",
                shortcut: Some("R"),
                description: "Skin-protecting Vibrance boosts muted tones gently, while Saturation scales chroma uniformly across the full gamut.",
                cli_key: "vibrance [-1..1]  ·  saturation [-1..1]",
                tier: "Tier 0 · Color",
            },
            Some(presence_reset),
            presence_controls,
        ))
        .child(bw_section(s, k))
        .child(hsl_editor_section(s, k))
}

fn detail_tab_content(s: &AppState, k: &Kit) -> El {
    let r = &s.recipe;

    // 1. CLARITY & DEHAZE (Clarity Method + Clarity / Structure / Dehaze)
    let clarity_neutral = r.clarity_method == ClarityMethod::Natural
        && r.clarity.abs() < 0.005
        && r.structure.abs() < 0.005
        && r.dehaze.abs() < 0.005;

    let mut method_row = row()
        .key("clarity-method-row")
        .role("radiogroup")
        .label("Clarity method")
        .w_full()
        .gap(space::PILL_GAP);
    for method in ClarityMethod::ALL {
        let active = r.clarity_method == method;
        method_row = method_row.child(crop_pill_btn(
            k,
            format!("clarity-method-{}", method.cli_slug()),
            "radio",
            format!("Clarity method {}", method.label()),
            method.label(),
            size::PILL_COMPACT_H,
            active,
            k.c.text_muted,
            move |s, tx| s.set_clarity_method_tx(method, tx),
        ));
    }

    let clarity_method_mod = usize::from(r.clarity_method != ClarityMethod::Natural);
    let mut clarity_controls = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(develop_scalar_slider(
            k,
            "sl-clarity",
            "clarity",
            "Clarity",
            r.clarity,
            format!("{:+.0}", r.clarity * 100.0),
            |rec, v| rec.clarity = v,
        ))
        .child(develop_scalar_slider(
            k,
            "sl-structure",
            "structure",
            "Structure",
            r.structure,
            format!("{:+.0}", r.structure * 100.0),
            |rec, v| rec.structure = v,
        ))
        .child(develop_scalar_slider(
            k,
            "sl-dehaze",
            "dehaze",
            "Dehaze",
            r.dehaze,
            format!("{:+.0}", r.dehaze * 100.0),
            |rec, v| rec.dehaze = v,
        ))
        .child(advanced_disclosure_row(
            k,
            "adv-clarity-method",
            "Clarity Method",
            s.clarity_advanced_open,
            clarity_method_mod,
            4,
            |s| s.clarity_advanced_open = !s.clarity_advanced_open,
        ));
    if s.clarity_advanced_open {
        clarity_controls = clarity_controls.child(method_row);
    }

    let clarity_reset = mini_reset_btn(k, "Reset Clarity & Dehaze", clarity_neutral, |s, tx| {
        s.update_slider_tx(true, tx, |rec| {
            rec.clarity_method = ClarityMethod::Natural;
            rec.clarity = scalar_default("clarity");
            rec.structure = scalar_default("structure");
            rec.dehaze = scalar_default("dehaze");
        });
    });

    // 2. SHARPENING (Amount at Tier 0; Radius, Threshold, Halo Suppression in Tier 1 Precision)
    let def_radius = scalar_default("sharpen_radius");
    let sharpen_neutral = r.sharpen.abs() < 0.005
        && (r.sharpen_radius - def_radius).abs() < 0.01
        && r.sharpen_threshold.abs() < 0.005
        && r.sharpen_halo.abs() < 0.005;
    let sharpen_adv_mod = usize::from((r.sharpen_radius - def_radius).abs() > 0.01)
        + usize::from(r.sharpen_threshold.abs() > 0.005)
        + usize::from(r.sharpen_halo.abs() > 0.005);
    let mut sharpen_controls = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(develop_scalar_slider(
            k,
            "sl-sharpen",
            "sharpen",
            "Amount",
            r.sharpen,
            format!("{:.0}", r.sharpen * 100.0),
            |rec, v| rec.sharpen = v,
        ))
        .child(advanced_disclosure_row(
            k,
            "adv-sharpen-precision",
            "Precision",
            s.sharpen_advanced_open,
            sharpen_adv_mod,
            3,
            |s| s.sharpen_advanced_open = !s.sharpen_advanced_open,
        ));
    if s.sharpen_advanced_open {
        sharpen_controls = sharpen_controls
            .child(develop_scalar_slider(
                k,
                "sl-sharpen-radius",
                "sharpen_radius",
                "Radius",
                r.sharpen_radius,
                format!("{:.1} px", r.sharpen_radius),
                |rec, v| rec.sharpen_radius = v,
            ))
            .child(develop_scalar_slider(
                k,
                "sl-sharpen-threshold",
                "sharpen_threshold",
                "Threshold",
                r.sharpen_threshold,
                format!("{:.0}", r.sharpen_threshold * 100.0),
                |rec, v| rec.sharpen_threshold = v,
            ))
            .child(develop_scalar_slider(
                k,
                "sl-sharpen-halo",
                "sharpen_halo",
                "Halo Suppression",
                r.sharpen_halo,
                format!("{:.0}", r.sharpen_halo * 100.0),
                |rec, v| rec.sharpen_halo = v,
            ));
    }
    let sharpen_reset = mini_reset_btn(k, "Reset Sharpening", sharpen_neutral, |s, tx| {
        s.update_slider_tx(true, tx, |rec| {
            rec.sharpen = scalar_default("sharpen");
            rec.sharpen_radius = scalar_default("sharpen_radius");
            rec.sharpen_threshold = scalar_default("sharpen_threshold");
            rec.sharpen_halo = scalar_default("sharpen_halo");
        });
    });

    // 3. NOISE REDUCTION (Luminance, Color)
    let nr_neutral = r.noise_reduction.abs() < 0.005 && r.nr_color.abs() < 0.005;
    let nr_controls = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(develop_scalar_slider(
            k,
            "sl-denoise",
            "noise_reduction",
            "Luminance",
            r.noise_reduction,
            format!("{:.0}", r.noise_reduction * 100.0),
            |rec, v| rec.noise_reduction = v,
        ))
        .child(develop_scalar_slider(
            k,
            "sl-nr-color",
            "nr_color",
            "Color",
            r.nr_color,
            format!("{:.0}", r.nr_color * 100.0),
            |rec, v| rec.nr_color = v,
        ));
    let nr_reset = mini_reset_btn(k, "Reset Noise Reduction", nr_neutral, |s, tx| {
        s.update_slider_tx(true, tx, |rec| {
            rec.noise_reduction = scalar_default("noise_reduction");
            rec.nr_color = scalar_default("nr_color");
        });
    });

    // 4. FILM GRAIN (Impact at Tier 0; 6 Grain Types + Granularity in Tier 1 Grain Structure)
    let def_grain_size = scalar_default("grain_size");
    let grain_neutral = r.grain_type == GrainType::Fine
        && r.grain.abs() < 0.005
        && (r.grain_size - def_grain_size).abs() < 0.005;
    let grain_adv_mod = usize::from(r.grain_type != GrainType::Fine)
        + usize::from((r.grain_size - def_grain_size).abs() > 0.005);
    let mut grain_type_grid = col()
        .key("grain-type-grid")
        .role("radiogroup")
        .label("Film grain type")
        .w_full()
        .gap(space::PILL_GAP);
    for chunk in GrainType::ALL.chunks(3) {
        let mut row_el = row().w_full().gap(space::PILL_GAP);
        for &gt in chunk {
            let active = r.grain_type == gt;
            row_el = row_el.child(crop_pill_btn(
                k,
                format!("grain-type-{}", gt.cli_slug()),
                "radio",
                format!("Film grain type {}", gt.label()),
                gt.label(),
                size::PILL_COMPACT_H,
                active,
                k.c.text_muted,
                move |s, tx| s.set_grain_type_tx(gt, tx),
            ));
        }
        grain_type_grid = grain_type_grid.child(row_el);
    }

    let mut grain_controls = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(develop_scalar_slider(
            k,
            "sl-grain",
            "grain",
            "Impact",
            r.grain,
            format!("{:.0}", r.grain * 100.0),
            |rec, v| rec.grain = v,
        ))
        .child(advanced_disclosure_row(
            k,
            "adv-grain-structure",
            "Grain Structure",
            s.grain_advanced_open,
            grain_adv_mod,
            7,
            |s| s.grain_advanced_open = !s.grain_advanced_open,
        ));
    if s.grain_advanced_open {
        grain_controls = grain_controls
            .child(grain_type_grid)
            .child(develop_scalar_slider(
                k,
                "sl-grain-size",
                "grain_size",
                "Granularity",
                r.grain_size,
                format!("{:.0}", r.grain_size * 100.0),
                |rec, v| rec.grain_size = v,
            ));
    }
    let grain_reset = mini_reset_btn(k, "Reset Film Grain", grain_neutral, |s, tx| {
        s.update_slider_tx(true, tx, |rec| {
            rec.grain_type = GrainType::Fine;
            rec.grain = scalar_default("grain");
            rec.grain_size = scalar_default("grain_size");
        });
    });

    // 5. VIGNETTING (Amount at Tier 0; Crop-aware Method in Tier 1)
    let vignette_neutral =
        r.vignette.abs() < 0.005 && r.vignette_method == VignetteMethod::EllipticOnCrop;
    let vignette_adv_mod = usize::from(r.vignette_method != VignetteMethod::EllipticOnCrop);
    let mut vignette_method_row = row()
        .key("vignette-method-row")
        .role("radiogroup")
        .label("Vignette method")
        .w_full()
        .gap(space::PILL_GAP);
    for method in VignetteMethod::ALL {
        let active = r.vignette_method == method;
        vignette_method_row = vignette_method_row.child(crop_pill_btn(
            k,
            format!("vig-method-{}", method.cli_slug()),
            "radio",
            format!("Vignette method {}", method.label()),
            method.label(),
            size::PILL_COMPACT_H,
            active,
            k.c.text_muted,
            move |s, tx| s.set_vignette_method_tx(method, tx),
        ));
    }
    let mut vignette_controls = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(develop_scalar_slider(
            k,
            "sl-vignette",
            "vignette",
            "Amount",
            r.vignette,
            format!("{:+.0}", r.vignette * 100.0),
            |rec, v| rec.vignette = v,
        ))
        .child(advanced_disclosure_row(
            k,
            "adv-vignette-method",
            "Vignette Method",
            s.vignette_advanced_open,
            vignette_adv_mod,
            3,
            |s| s.vignette_advanced_open = !s.vignette_advanced_open,
        ));
    if s.vignette_advanced_open {
        vignette_controls = vignette_controls.child(vignette_method_row);
    }
    let vignette_reset = mini_reset_btn(k, "Reset Vignetting", vignette_neutral, |s, tx| {
        s.update_slider_tx(true, tx, |rec| {
            rec.vignette = scalar_default("vignette");
            rec.vignette_method = VignetteMethod::EllipticOnCrop;
        });
    });

    col()
        .w_full()
        .gap(space::CARD_GAP)
        .child(collapsible_section_card(
            k,
            "sec-clarity",
            "CLARITY & DEHAZE",
            s.clarity_open,
            !clarity_neutral,
            |s| s.clarity_open = !s.clarity_open,
            s.hovered_info,
            InfoTip {
                title: "Clarity, Structure & Dehaze",
                shortcut: Some("Z"),
                description: "Midtone local contrast (Clarity), fine micro-texture (Structure), and atmospheric contrast recovery (Dehaze) across 4 Capture One algorithms.",
                cli_key: "clarity  ·  structure  ·  dehaze  ·  clarity_method",
                tier: "Tier 0 · Detail",
            },
            Some(clarity_reset),
            clarity_controls,
        ))
        .child(collapsible_section_card(
            k,
            "sec-sharpen",
            "SHARPENING",
            s.sharpen_open,
            !sharpen_neutral,
            |s| s.sharpen_open = !s.sharpen_open,
            s.hovered_info,
            InfoTip {
                title: "Unsharp Mask & Halo Suppression",
                shortcut: None,
                description: "Edge-aware sharpening with configurable radius, flat-surface noise threshold, and bright-edge halo suppression.",
                cli_key: "sharpen  ·  sharpen_radius  ·  sharpen_threshold  ·  sharpen_halo",
                tier: "Tier 1 · Detail",
            },
            Some(sharpen_reset),
            sharpen_controls,
        ))
        .child(collapsible_section_card(
            k,
            "sec-nr",
            "NOISE REDUCTION",
            s.nr_open,
            !nr_neutral,
            |s| s.nr_open = !s.nr_open,
            s.hovered_info,
            InfoTip {
                title: "Noise Reduction",
                shortcut: None,
                description: "Separates edge-preserving luminance grain smoothing from chroma color-blotch denoising.",
                cli_key: "noise_reduction [0..1]  ·  nr_color [0..1]",
                tier: "Tier 1 · Detail",
            },
            Some(nr_reset),
            nr_controls,
        ))
        .child(collapsible_section_card(
            k,
            "sec-grain",
            "FILM GRAIN",
            s.grain_open,
            !grain_neutral,
            |s| s.grain_open = !s.grain_open,
            s.hovered_info,
            InfoTip {
                title: "Silver-Halide Film Grain",
                shortcut: None,
                description: "Organic luminance-weighted grain across 6 analog emulsion profiles (Fine, Silver Rich, Soft, Cubic, Tabular, Harsh).",
                cli_key: "grain  ·  grain_size  ·  grain_type",
                tier: "Tier 1 · Texture",
            },
            Some(grain_reset),
            grain_controls,
        ))
        .child(collapsible_section_card(
            k,
            "sec-vignette",
            "VIGNETTING",
            s.vignette_open,
            !vignette_neutral,
            |s| s.vignette_open = !s.vignette_open,
            s.hovered_info,
            InfoTip {
                title: "Optical Vignetting",
                shortcut: None,
                description: "Subtle natural corner darkening or brightening anchored to the active crop rectangle or full frame.",
                cli_key: "vignette [-1..1]  ·  vignette_method",
                tier: "Tier 1 · Optics",
            },
            Some(vignette_reset),
            vignette_controls,
        ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn crop_pill_btn(
    k: &Kit,
    key: impl Into<String>,
    role: &'static str,
    aria_label: impl Into<String>,
    text_label: &'static str,
    height: f32,
    active: bool,
    rest_text_color: Color,
    on_press: impl Fn(&mut AppState, &mut Tx) + 'static,
) -> El {
    let mut pill = row()
        .key(key.into())
        .role(role)
        .label(aria_label.into())
        .focusable(true)
        .focus_ring(
            border::FOCUS_RING,
            border::FOCUS_OFFSET_TIGHT,
            k.c.border_focus,
        )
        .cursor(Cursor::Pointer)
        .grow(1.0)
        .min_w(0.0)
        .h(height)
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
        .child(
            text(text_label)
                .font(ty::section_header(k))
                .color(if active {
                    k.c.text_accent
                } else {
                    rest_text_color
                }),
        )
        .on_press_tx(on_press);
    if role == "radio" {
        pill = pill.checked(active);
    }
    pill
}

fn crop_tab_content(s: &AppState, k: &Kit) -> El {
    let geo = s.recipe.geometry;
    let [out_w, out_h] = geo.cropped_output_size(s.image.width, s.image.height);

    let build_aspect_row = |presets: &[CropAspect]| {
        let mut r = row().w_full().gap(space::PILL_GAP);
        for &preset in presets {
            let active = geo.aspect_preset == preset;
            r = r.child(crop_pill_btn(
                k,
                format!("aspect-{}", preset.label()),
                "radio",
                format!("Crop aspect {}", preset.label()),
                preset.label(),
                size::ASPECT_PILL_H,
                active,
                k.c.text_muted,
                move |s, tx| s.set_crop_aspect_tx(preset, tx),
            ));
        }
        r
    };
    let primary_aspects = &CropAspect::ALL[..4.min(CropAspect::ALL.len())];
    let more_aspects = &CropAspect::ALL[4.min(CropAspect::ALL.len())..];
    let more_aspect_active = usize::from(more_aspects.contains(&geo.aspect_preset));

    let crop_toggle = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(
            col()
                .gap(space::MICRO)
                .child(
                    text("Interactive Crop Frame")
                        .font(ty::control_label(k))
                        .color(k.c.text_base),
                )
                .child(
                    text(format!("Output {out_w}×{out_h} px"))
                        .font(ty::mono_compact(k))
                        .color(k.c.text_muted),
                ),
        )
        .child(
            Switch::new(s.camera.crop_mode)
                .label("Interactive Crop Frame")
                .hide_label()
                .on_toggle_tx(|s: &mut AppState, tx| s.toggle_crop_mode_tx(tx))
                .build(k),
        );

    let crop_reset = mini_reset_btn(k, "Reset Crop & Geometry", geo.is_identity(), |s, tx| {
        s.update_geometry_slider_tx(true, tx, |rec| rec.geometry.reset());
    });

    let mut crop_body = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(crop_toggle)
        .child(build_aspect_row(primary_aspects));
    if !more_aspects.is_empty() {
        crop_body = crop_body.child(advanced_disclosure_row(
            k,
            "adv-crop-ratios",
            "More Ratios",
            s.crop_advanced_open,
            more_aspect_active,
            more_aspects.len(),
            |s| s.crop_advanced_open = !s.crop_advanced_open,
        ));
        if s.crop_advanced_open {
            for chunk in more_aspects.chunks(4) {
                crop_body = crop_body.child(build_aspect_row(chunk));
            }
        }
    }

    let straighten = develop_slider_inner(
        k,
        "sl-straighten",
        "Straighten",
        None,
        geo.sanitized_straighten_deg(),
        -45.0..=45.0,
        format!("{:+.1}°", geo.sanitized_straighten_deg()),
        0.0,
        false,
        |rec, v| rec.geometry.straighten_deg = v,
    );

    let rot_btn = |key: &'static str,
                   label: &'static str,
                   active: bool,
                   on_press: fn(&mut AppState, &mut Tx)| {
        crop_pill_btn(
            k,
            key,
            "button",
            label,
            label,
            size::ACTION_PILL_H,
            active,
            k.c.text_base,
            on_press,
        )
    };

    let xform_row = row()
        .w_full()
        .gap(space::PILL_GAP)
        .child(rot_btn("btn-rot-ccw", "↺ 90°", false, |s, tx| {
            s.update_geometry_slider_tx(true, tx, |r| {
                r.geometry.rotate_90 = (r.geometry.rotate_90 - 1).rem_euclid(4);
            });
        }))
        .child(rot_btn("btn-rot-cw", "↻ 90°", false, |s, tx| {
            s.update_geometry_slider_tx(true, tx, |r| {
                r.geometry.rotate_90 = (r.geometry.rotate_90 + 1).rem_euclid(4);
            });
        }))
        .child(rot_btn("btn-flip-h", "Flip H", geo.flip_h, |s, tx| {
            s.update_geometry_slider_tx(true, tx, |r| r.geometry.flip_h = !r.geometry.flip_h);
        }))
        .child(rot_btn("btn-flip-v", "Flip V", geo.flip_v, |s, tx| {
            s.update_geometry_slider_tx(true, tx, |r| r.geometry.flip_v = !r.geometry.flip_v);
        }));

    let rot_body = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(straighten)
        .child(xform_row);

    let grid_idx = CompositionGrid::ALL
        .iter()
        .position(|&g| g == geo.grid)
        .unwrap_or(1);
    let grid_labels = ["Off", "Thirds", "Golden", "Diag"];
    let grid_seg = SegmentedControl::new(&grid_labels, Some(grid_idx))
        .label("Composition grid overlay")
        .width(None)
        .on_pick(|s: &mut AppState, i, tx| {
            let next_grid = CompositionGrid::ALL
                .get(i)
                .copied()
                .unwrap_or(CompositionGrid::RuleOfThirds);
            s.update_geometry_slider_tx(true, tx, |r| r.geometry.grid = next_grid);
        })
        .build(k);

    let ks_neutral = geo.is_keystone_identity();
    let ks_adv_mod = usize::from(geo.keystone_aspect.abs() > 0.005)
        + usize::from(
            (geo.keystone_amount - descriptor_default("geometry.keystone_amount")).abs() > 0.01,
        );
    let mut ks_body = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(develop_slider_inner(
            k,
            "sl-ks-vertical",
            "Vertical",
            None,
            geo.keystone_vertical,
            descriptor_range("geometry.keystone_vertical"),
            format!("{:+.0}", geo.keystone_vertical * 100.0),
            descriptor_default("geometry.keystone_vertical"),
            false,
            |r, v| r.geometry.keystone_vertical = v,
        ))
        .child(develop_slider_inner(
            k,
            "sl-ks-horizontal",
            "Horizontal",
            None,
            geo.keystone_horizontal,
            descriptor_range("geometry.keystone_horizontal"),
            format!("{:+.0}", geo.keystone_horizontal * 100.0),
            descriptor_default("geometry.keystone_horizontal"),
            false,
            |r, v| r.geometry.keystone_horizontal = v,
        ))
        .child(advanced_disclosure_row(
            k,
            "adv-keystone-fine",
            "Fine Keystone",
            s.keystone_advanced_open,
            ks_adv_mod,
            2,
            |s| s.keystone_advanced_open = !s.keystone_advanced_open,
        ));
    if s.keystone_advanced_open {
        ks_body = ks_body
            .child(develop_slider_inner(
                k,
                "sl-ks-aspect",
                "Aspect",
                None,
                geo.keystone_aspect,
                descriptor_range("geometry.keystone_aspect"),
                format!("{:+.0}", geo.keystone_aspect * 100.0),
                descriptor_default("geometry.keystone_aspect"),
                false,
                |r, v| r.geometry.keystone_aspect = v,
            ))
            .child(develop_slider_inner(
                k,
                "sl-ks-amount",
                "Amount",
                None,
                geo.keystone_amount,
                descriptor_range("geometry.keystone_amount"),
                format!("{:.0}%", geo.keystone_amount * 100.0),
                descriptor_default("geometry.keystone_amount"),
                false,
                |r, v| r.geometry.keystone_amount = v,
            ));
    }
    let ks_reset = mini_reset_btn(k, "Reset Keystone", ks_neutral, |s, tx| {
        s.update_geometry_slider_tx(true, tx, |r| {
            r.geometry.keystone_vertical = descriptor_default("geometry.keystone_vertical");
            r.geometry.keystone_horizontal = descriptor_default("geometry.keystone_horizontal");
            r.geometry.keystone_aspect = descriptor_default("geometry.keystone_aspect");
            r.geometry.keystone_amount = descriptor_default("geometry.keystone_amount");
        });
    });

    let lens = s.recipe.lens.sanitized();
    let lens_neutral = lens.is_identity();
    let lens_adv_mod = usize::from(lens.purple_fringing.abs() > 0.005)
        + usize::from(lens.light_falloff.abs() > 0.005);
    let mut lens_body = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(develop_slider_inner(
            k,
            "sl-lens-distortion",
            "Distortion",
            None,
            lens.distortion,
            descriptor_range("lens.distortion"),
            format!("{:+.0}", lens.distortion * 100.0),
            descriptor_default("lens.distortion"),
            false,
            |r, v| r.lens.distortion = v,
        ))
        .child(develop_slider_inner(
            k,
            "sl-lens-ca",
            "Chromatic Aberration",
            None,
            lens.chromatic_aberration,
            descriptor_range("lens.chromatic_aberration"),
            format!("{:+.0}", lens.chromatic_aberration * 100.0),
            descriptor_default("lens.chromatic_aberration"),
            false,
            |r, v| r.lens.chromatic_aberration = v,
        ))
        .child(advanced_disclosure_row(
            k,
            "adv-lens-edge",
            "Edge Cleanup",
            s.lens_advanced_open,
            lens_adv_mod,
            2,
            |s| s.lens_advanced_open = !s.lens_advanced_open,
        ));
    if s.lens_advanced_open {
        lens_body = lens_body
            .child(develop_slider_inner(
                k,
                "sl-lens-fringe",
                "Purple Fringing",
                None,
                lens.purple_fringing,
                descriptor_range("lens.purple_fringing"),
                format!("{:.0}", lens.purple_fringing * 100.0),
                descriptor_default("lens.purple_fringing"),
                true,
                |r, v| r.lens.purple_fringing = v,
            ))
            .child(develop_slider_inner(
                k,
                "sl-lens-falloff",
                "Light Falloff",
                None,
                lens.light_falloff,
                descriptor_range("lens.light_falloff"),
                format!("{:.0}", lens.light_falloff * 100.0),
                descriptor_default("lens.light_falloff"),
                true,
                |r, v| r.lens.light_falloff = v,
            ));
    }
    let lens_reset = mini_reset_btn(k, "Reset Lens Correction", lens_neutral, |s, tx| {
        s.update_slider_tx(true, tx, |r| {
            r.lens = LensParams::default();
        });
    });

    let crop_modified = geo.crop != [0.0, 0.0, 1.0, 1.0] || geo.aspect_preset != CropAspect::Free;
    let rot_modified = geo.sanitized_straighten_deg().abs() > 0.01
        || geo.rotate_90 != 0
        || geo.flip_h
        || geo.flip_v;

    col()
        .w_full()
        .gap(space::CARD_GAP)
        .child(collapsible_section_card(
            k,
            "sec-crop",
            "CROP & ASPECT",
            s.crop_open,
            crop_modified,
            |s| s.crop_open = !s.crop_open,
            s.hovered_info,
            InfoTip {
                title: "Crop & Aspect Ratio",
                shortcut: Some("C"),
                description: "Non-destructive crop frame with aspect presets. Drag corners or edges directly on the photo, then press C to apply.",
                cli_key: "geometry.{crop,aspect_preset}",
                tier: "Tier 0 · Geometry",
            },
            Some(crop_reset),
            crop_body,
        ))
        .child(collapsible_section_card(
            k,
            "sec-rotate",
            "ROTATE & FLIP",
            s.rotate_open,
            rot_modified,
            |s| s.rotate_open = !s.rotate_open,
            s.hovered_info,
            InfoTip {
                title: "Rotate, Straighten & Flip",
                shortcut: None,
                description: "Continuous horizon straighten (-45° to +45°) paired with lossless 90° quarter-turns and horizontal/vertical flips.",
                cli_key: "geometry.{straighten_deg,rotate_90,flip_h,flip_v}",
                tier: "Tier 0 · Geometry",
            },
            None,
            rot_body,
        ))
        .child(collapsible_section_card(
            k,
            "sec-keystone",
            "KEYSTONE",
            s.keystone_open,
            !ks_neutral,
            |s| s.keystone_open = !s.keystone_open,
            s.hovered_info,
            InfoTip {
                title: "Keystone Perspective Homography",
                shortcut: None,
                description: "Corrects converging vertical and horizontal architectural lines with aspect compensation and strength blending.",
                cli_key: "geometry.keystone_{vertical,horizontal,aspect,amount}",
                tier: "Tier 1 · Optics",
            },
            Some(ks_reset),
            ks_body,
        ))
        .child(collapsible_section_card(
            k,
            "sec-lens",
            "LENS CORRECTION",
            s.lens_open,
            !lens_neutral,
            |s| s.lens_open = !s.lens_open,
            s.hovered_info,
            InfoTip {
                title: "Optical Lens Correction",
                shortcut: None,
                description: "Brown-Conrady barrel/pincushion distortion, lateral chromatic aberration, purple fringing, and lens light falloff recovery.",
                cli_key: "lens.{distortion,chromatic_aberration,purple_fringing,light_falloff}",
                tier: "Tier 1 · Optics",
            },
            Some(lens_reset),
            lens_body,
        ))
        .child(collapsible_section_card(
            k,
            "sec-grid",
            "COMPOSITION GRID",
            s.grid_open,
            geo.grid != CompositionGrid::None,
            |s| s.grid_open = !s.grid_open,
            s.hovered_info,
            InfoTip {
                title: "Composition Grid Overlay",
                shortcut: None,
                description: "Displays Rule of Thirds, Golden Ratio, or Diagonal guides over the photo viewport (also toggled in the titlebar Overlays ▾ popover).",
                cli_key: "geometry.grid",
                tier: "Tier 1 · Guides",
            },
            None,
            grid_seg,
        ))
}

fn presets_tab_content(s: &AppState, k: &Kit) -> El {
    // 1. VIRTUAL VARIANTS + COMPARE VARIANT
    let v_count = s.active_variant_count();
    let v_active = s.active_variant_index();
    let mut variant_pills = row().w_full().gap(space::XS);
    for idx in 0..v_count.min(4) {
        let active = idx == v_active;
        variant_pills = variant_pills.child(crop_pill_btn(
            k,
            format!("style-var-{idx}"),
            "radio",
            format!("Select Variant V{}", idx + 1),
            match idx {
                0 => "V1",
                1 => "V2",
                2 => "V3",
                _ => "V4",
            },
            size::PILL_COMPACT_H,
            active,
            k.c.text_muted,
            move |s, tx| s.select_variant_tx(idx, tx),
        ));
    }
    let variant_actions = row()
        .w_full()
        .gap(space::XS)
        .child(crop_pill_btn(
            k,
            "btn-var-new",
            "button",
            "Create New Neutral Variant",
            "+ New",
            size::PILL_H,
            false,
            k.c.text_base,
            |s, tx| s.new_variant_tx(None, tx),
        ))
        .child(crop_pill_btn(
            k,
            "btn-var-clone",
            "button",
            "Clone Active Variant",
            "Clone",
            size::PILL_H,
            false,
            k.c.text_base,
            |s, tx| s.clone_variant_tx(None, tx),
        ))
        .child(crop_pill_btn(
            k,
            "btn-var-compare",
            "button",
            "Pin or Clear Compare Variant",
            if s.compare_reference.is_some() {
                "Comparing"
            } else {
                "Compare"
            },
            size::PILL_H,
            s.compare_reference.is_some(),
            k.c.text_base,
            |s, tx| s.toggle_compare_variant_tx(tx),
        ));
    let variants_card = collapsible_section_card(
        k,
        "sec-variants",
        "VIRTUAL VARIANTS",
        s.variants_open,
        v_count > 1 || s.compare_reference.is_some(),
        |s| s.variants_open = !s.variants_open,
        s.hovered_info,
        InfoTip {
            title: "Virtual Variants",
            shortcut: Some("⌘⏎"),
            description: "Stores multiple independent non-destructive looks inside one photo sidecar. Press ⌘⏎ to pin the active variant as a side-by-side Compare Reference.",
            cli_key: "MCP: rune_manage_workflow (new/clone/select/compare variant)",
            tier: "Tier 1 · Workflow",
        },
        Some(
            text(format!("V{}/{v_count}", v_active + 1))
                .font(ty::mono_compact(k))
                .color(k.c.text_faint),
        ),
        col()
            .w_full()
            .gap(space::ROW_GAP)
            .child(variant_pills)
            .child(variant_actions),
    );

    // 2. BUILT-IN STYLES + STACK TOGGLE & STRENGTH CAPSULE
    let mut grid_rows = col().w_full().gap(space::SM);
    for chunk in s.presets.chunks(2) {
        let mut r = row().w_full().gap(space::SM);
        for preset in chunk {
            let active = !s.stack_styles && s.recipe.preset_name.as_deref() == Some(preset.name);
            let id = preset.id;
            let name = preset.name;
            let subtitle = if preset.recipe.bw.enabled || preset.recipe.saturation <= -0.95 {
                "Monochrome"
            } else if preset.recipe.temperature > 0.1 {
                "Warm Analog"
            } else if preset.recipe.clarity > 0.18 {
                "Crisp Studio"
            } else if preset.recipe.is_neutral() {
                "Linear Reset"
            } else {
                "Editorial Color"
            };

            let card = col()
                .key(format!("preset-{id}"))
                .role("button")
                .label(format!("Apply style {name}"))
                .focusable(true)
                .focus_ring(
                    border::FOCUS_RING,
                    border::FOCUS_OFFSET_TIGHT,
                    k.c.border_focus,
                )
                .cursor(Cursor::Pointer)
                .grow(1.0)
                .min_w(0.0)
                .gap(space::XXS)
                .padding(space::MD)
                .radius(k.radius.md)
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
                .child(
                    text(name)
                        .font(ty::control_label_medium(k))
                        .color(if active {
                            k.c.text_accent
                        } else {
                            k.c.text_base
                        }),
                )
                .child(text(subtitle).font(ty::caption(k)).color(k.c.text_faint))
                .on_press_tx(move |s: &mut AppState, tx| s.apply_preset_tx(id, tx));

            r = r.child(card);
        }
        grid_rows = grid_rows.child(r);
    }

    let stack_row = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(
            text("Stack Styles (Additive Blend)")
                .font(ty::control_label(k))
                .color(if s.stack_styles {
                    k.c.text_base
                } else {
                    k.c.text_muted
                }),
        )
        .child(
            Switch::new(s.stack_styles)
                .label("Stack Styles cumulatively")
                .hide_label()
                .on_toggle(|s: &mut AppState| s.stack_styles = !s.stack_styles)
                .build(k),
        );

    let strength_w = size::CARD_INNER_W;
    let strength_fill = (s.style_strength.clamp(0.0, 1.0) * strength_w).max(0.0);
    let strength_capsule = el()
        .key("sl-style-strength")
        .role("slider")
        .label("Style strength")
        .cursor(Cursor::ResizeColumn)
        .w(strength_w)
        .h(size::CAPSULE_SLIDER_H)
        .radius(k.radius.lg)
        .fill(color::capsule_bg(k))
        .hover(color::capsule_hover(k))
        .border(border::HAIRLINE, k.c.border_base)
        .clip_rounded(k.radius.lg)
        .child(
            el().absolute(0.0, 0.0)
                .size(strength_fill, size::CAPSULE_SLIDER_H)
                .fill(color::capsule_fill(k))
                .a11y_hidden(),
        )
        .child(
            row()
                .w_full()
                .h_full()
                .px(space::LG)
                .items_center()
                .justify_between()
                .child(
                    text("Strength")
                        .font(ty::control_label(k))
                        .color(k.c.text_muted),
                )
                .child(
                    text(format!("{:.0}%", s.style_strength * 100.0))
                        .font(ty::mono_value(k))
                        .color(k.c.text_base),
                ),
        )
        .on_drag(|s: &mut AppState, d: &mut Drag, _| {
            if d.clicks >= 2 && d.phase == Phase::Down {
                s.style_strength = 1.0;
                return;
            }
            if matches!(d.phase, Phase::Down | Phase::Move | Phase::Up) {
                s.style_strength = (d.x / d.w.max(1.0)).clamp(0.0, 1.0);
            }
        });

    let styles_body = col().w_full().gap(space::SM).child(grid_rows);

    // 3. USER STYLES & STACKING
    let save_row = row()
        .w_full()
        .items_center()
        .gap(space::XS)
        .child(
            el().grow(1.0).min_w(0.0).child(
                TextInput::new(&s.style_name_edit)
                    .placeholder("Style name…")
                    .width(None)
                    .on_edit(|s: &mut AppState| &mut s.style_name_edit)
                    .on_submit(|s: &mut AppState, tx| s.save_current_as_user_style_tx(tx))
                    .build(k),
            ),
        )
        .child(
            Button::new("Save")
                .variant(ButtonVariant::Neutral)
                .control_size(ControlSize::Small)
                .on_press_tx(|s: &mut AppState, tx| s.save_current_as_user_style_tx(tx))
                .build(k),
        );

    let stack_mod =
        usize::from(s.stack_styles) + usize::from((s.style_strength - 1.0).abs() > 0.01);
    let mut user_styles_body = col().w_full().gap(space::ROW_GAP).child(save_row);
    if s.user_styles.is_empty() {
        user_styles_body = user_styles_body.child(
            text("No saved user styles yet.")
                .font(ty::caption(k))
                .color(k.c.text_faint),
        );
    } else {
        let mut list = col().w_full().gap(space::PILL_GAP);
        for (idx, st) in s.user_styles.iter().enumerate() {
            let item = row()
                .key(format!("user-style-{idx}"))
                .w_full()
                .items_center()
                .justify_between()
                .gap(space::XS)
                .px(space::MD)
                .py(space::SM)
                .radius(k.radius.sm)
                .fill(k.c.bg_base)
                .hover(k.c.hover)
                .border(border::HAIRLINE, k.c.border_base)
                .cursor(Cursor::Pointer)
                .child(
                    col()
                        .min_w(0.0)
                        .shrink()
                        .child(
                            text(st.name.as_str())
                                .font(ty::control_label_medium(k))
                                .color(k.c.text_base)
                                .truncate(),
                        )
                        .child(
                            text(format!("{} group(s)", st.groups.len()))
                                .font(ty::mono_micro(k))
                                .color(k.c.text_faint),
                        ),
                )
                .child(
                    IconButton::new(&icons::CLOSE)
                        .variant(IconButtonVariant::GhostMuted)
                        .control_size(ControlSize::Mini)
                        .label(format!("Delete style {}", st.name))
                        .on_press_tx(move |s: &mut AppState, tx| s.delete_user_style_tx(idx, tx))
                        .build(k),
                )
                .on_press_tx(move |s: &mut AppState, tx| s.apply_user_style_tx(idx, tx));
            list = list.child(item);
        }
        user_styles_body = user_styles_body.child(list);
    }
    user_styles_body = user_styles_body.child(advanced_disclosure_row(
        k,
        "adv-user-styles-blend",
        "Blending & Stacking",
        s.user_styles_advanced_open,
        stack_mod,
        2,
        |s| s.user_styles_advanced_open = !s.user_styles_advanced_open,
    ));
    if s.user_styles_advanced_open {
        user_styles_body = user_styles_body.child(stack_row).child(strength_capsule);
    }

    // 4. SELECTIVE ADJUSTMENTS CLIPBOARD
    let mut group_grid = col().w_full().gap(space::XS);
    for chunk in ALL_CLIPBOARD_GROUPS.chunks(2) {
        let mut r = row().w_full().gap(space::XS);
        for &(group_id, group_label) in chunk {
            let checked = s.clipboard_groups.contains(group_id);
            let cell = row()
                .key(format!("clip-grp-{group_id}"))
                .grow(1.0)
                .min_w(0.0)
                .items_center()
                .gap(space::SM)
                .px(space::SM)
                .py(space::XS)
                .radius(k.radius.sm)
                .fill(if checked { k.c.bg_base } else { k.c.bg_deep })
                .border(
                    border::HAIRLINE,
                    if checked {
                        k.c.border_strong
                    } else {
                        k.c.border_muted
                    },
                )
                .cursor(Cursor::Pointer)
                .child(
                    Checkbox::new(checked)
                        .label(group_label)
                        .hide_label()
                        .on_toggle(move |s: &mut AppState| s.toggle_clipboard_group(group_id))
                        .build(k),
                )
                .child(
                    text(group_label)
                        .font(ty::caption(k))
                        .color(if checked {
                            k.c.text_base
                        } else {
                            k.c.text_muted
                        })
                        .truncate(),
                )
                .on_press(move |s: &mut AppState| s.toggle_clipboard_group(group_id));
            r = r.child(cell);
        }
        group_grid = group_grid.child(r);
    }

    let autoselect_btn = Button::new("Autoselect Adjusted")
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .on_press(|s: &mut AppState| s.autoselect_adjusted_groups())
        .build(k);

    let clipboard_actions = row()
        .w_full()
        .gap(space::SM)
        .child(
            Button::new("Copy")
                .variant(ButtonVariant::Neutral)
                .control_size(ControlSize::Small)
                .icon(&icons::COPY)
                .shortcut("cmd+shift+c")
                .on_press_tx(|s: &mut AppState, tx| s.copy_adjustments_tx(tx))
                .build(k),
        )
        .child(
            Button::new("Apply Selected")
                .variant(ButtonVariant::Contrast)
                .control_size(ControlSize::Small)
                .shortcut("cmd+shift+v")
                .disabled(!s.catalog.has_clipboard() || s.clipboard_groups.is_empty())
                .on_press_tx(|s: &mut AppState, tx| s.paste_selected_adjustments_tx(tx))
                .build(k),
        );

    let reset_all_row = row().w_full().gap(space::MD).child(
        Button::new("Reset All Adjustments")
            .variant(ButtonVariant::Danger)
            .control_size(ControlSize::Regular)
            .icon(&icons::OUTLINE_RESET)
            .shortcut("cmd+r")
            .disabled(s.recipe.is_neutral())
            .on_press_tx(|s: &mut AppState, tx| s.reset_recipe_tx(tx))
            .build(k),
    );

    let mut clipboard_body = col()
        .w_full()
        .gap(space::ROW_GAP)
        .child(clipboard_actions)
        .child(autoselect_btn)
        .child(advanced_disclosure_row(
            k,
            "adv-clipboard-groups",
            "Selective Groups",
            s.clipboard_groups_open,
            s.clipboard_groups.len(),
            ALL_CLIPBOARD_GROUPS.len(),
            |s| s.clipboard_groups_open = !s.clipboard_groups_open,
        ));
    if s.clipboard_groups_open {
        clipboard_body = clipboard_body.child(group_grid).child(reset_all_row);
    }

    col()
        .w_full()
        .gap(space::CARD_GAP)
        .child(collapsible_section_card(
            k,
            "sec-styles",
            "STYLES & PRESETS",
            s.styles_open,
            s.recipe.preset_name.is_some(),
            |s| s.styles_open = !s.styles_open,
            s.hovered_info,
            InfoTip {
                title: "Built-In Darkroom Styles",
                shortcut: None,
                description: "Non-destructive calibrated film and editorial looks. Enable Stack Styles under User Styles to blend multiple styles additively.",
                cli_key: "rd edit <photo> --preset <id> --mix 0.8",
                tier: "Tier 0 · Styles",
            },
            None,
            styles_body,
        ))
        .child(variants_card)
        .child(collapsible_section_card(
            k,
            "sec-user-styles",
            "USER STYLES",
            s.user_styles_open,
            !s.user_styles.is_empty(),
            |s| s.user_styles_open = !s.user_styles_open,
            s.hovered_info,
            InfoTip {
                title: "User Styles & Stacking",
                shortcut: None,
                description: "Save custom grades as reusable user styles in ~/.config/rune-darkroom/styles and stack them with adjustable blend strength.",
                cli_key: "MCP: rune_manage_workflow (save/apply/stack user styles)",
                tier: "Tier 1 · Styles",
            },
            Some(
                text(format!("{}", s.user_styles.len()))
                    .font(ty::mono_compact(k))
                    .color(k.c.text_faint),
            ),
            user_styles_body,
        ))
        .child(collapsible_section_card(
            k,
            "sec-clipboard",
            "ADJUSTMENTS CLIPBOARD",
            s.clipboard_open,
            s.catalog.has_clipboard(),
            |s| s.clipboard_open = !s.clipboard_open,
            s.hovered_info,
            InfoTip {
                title: "Selective Adjustments Clipboard",
                shortcut: Some("⌘⇧C / ⌘⇧V"),
                description: "Copy adjustments from the active photo and paste only selected groups (WB, Tone, Color, Detail, Layers, etc.) onto other shots.",
                cli_key: "MCP: rune_manage_workflow (copy/apply clipboard)",
                tier: "Tier 1 · Batch",
            },
            Some(
                text(format!(
                    "{}/{}",
                    s.clipboard_groups.len(),
                    ALL_CLIPBOARD_GROUPS.len()
                ))
                .font(ty::mono_compact(k))
                .color(k.c.text_faint),
            ),
            clipboard_body,
        ))
}

/// Complete tabbed Capture One / OpenCode right inspector with pinned Histogram and
/// native OpenCode scroll bar.
pub fn right_inspector(s: &AppState, k: &Kit) -> El {
    let mut tabs = row()
        .key("inspector-tabs")
        .role("tablist")
        .label("Inspector tabs")
        .w_full()
        .padding(space::XXS)
        .gap(space::XXS)
        .radius(k.radius.md)
        .fill(k.c.bg_deep)
        .border(border::HAIRLINE, k.c.border_base);
    for (idx, &label) in InspectorTab::LABELS.iter().enumerate() {
        let active = s.active_tab.index() == idx;
        tabs = tabs.child(
            row()
                .key(format!("inspector-tab-{idx}"))
                .role("tab")
                .label(label)
                .checked(active)
                .controls("inspector-tabpanel")
                .focusable(true)
                .focus_ring(
                    border::FOCUS_RING,
                    border::FOCUS_OFFSET_TIGHT,
                    k.c.border_focus,
                )
                .cursor(Cursor::Pointer)
                .grow(1.0)
                .min_w(0.0)
                .h(size::PILL_H)
                .px(space::XXS)
                .center()
                .radius(k.radius.sm)
                .fill(if active {
                    k.c.layer_03
                } else {
                    Color::TRANSPARENT
                })
                .hover(if active { k.c.layer_03 } else { k.c.hover })
                .border(
                    if active {
                        border::HAIRLINE
                    } else {
                        border::NONE
                    },
                    if active {
                        k.c.border_strong
                    } else {
                        Color::TRANSPARENT
                    },
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
                )
                .on_press(move |s: &mut AppState| {
                    s.set_active_tab(InspectorTab::from_index(idx));
                }),
        );
    }

    let pinned_header = col()
        .key("inspector-pinned")
        .w_full()
        .gap(space::CARD_HEADER_GAP)
        .padding(space::CARD_PAD)
        .border_sides(
            [border::NONE, border::NONE, border::REGULAR, border::NONE],
            k.c.border_base,
        )
        .child(histogram_panel(s, k))
        .child(tabs);

    let tab_body = match s.active_tab {
        InspectorTab::Light => light_tab_content(s, k),
        InspectorTab::Color => color_tab_content(s, k),
        InspectorTab::Detail => detail_tab_content(s, k),
        InspectorTab::Crop => crop_tab_content(s, k),
        InspectorTab::Layers => crate::layers_panel::layers_tab_content(s, k),
        InspectorTab::Presets => presets_tab_content(s, k),
    };

    let scroll_content = col()
        .key("inspector-tabpanel")
        .role("tabpanel")
        .label(format!(
            "{} adjustments",
            InspectorTab::LABELS[s.active_tab.index()]
        ))
        .w_full()
        .padding(space::CARD_PAD)
        .child(tab_body);

    let scroll_viewport = col()
        .key("inspector-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec::default())
        .scroll_edge_fade(size::SCROLL_FADE)
        .child(scroll_content);

    col()
        .key("develop-inspector")
        .w(size::INSPECTOR_W)
        .h_full()
        .min_h(0.0)
        .fill(k.c.bg_base)
        .border_sides(
            [border::NONE, border::NONE, border::NONE, border::REGULAR],
            k.c.border_base,
        )
        .child(pinned_header)
        .child(k.scroll_bar(scroll_viewport))
}
